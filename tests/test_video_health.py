"""Video watchdog ownership/recovery checks with scheduler/process calls mocked."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipUnless(os.name=="nt","Windows video scheduler")
class VideoHealthTests(unittest.TestCase):
    def run_helper(self, case, role="Receive"):
        with tempfile.TemporaryDirectory(prefix="usbbridge-video-health-") as directory:
            script=r"""
$ErrorActionPreference='Stop'
$testRole='ROLE';$testMode=if($testRole-eq 'Receive'){'receive'}else{'send'};$testCase='CASE';$runtime='RUNTIME';$config='CONFIG';$script:stopped=$false
$exe=Join-Path $runtime 'usbbridge-camera.exe'
# Intercept the exact application-state command without requiring build output
# or querying an installed application's state on the test machine.
Set-Item -LiteralPath ('Function:'+(Join-Path $runtime 'usbbridge.exe')) -Value {
 if($args.Count-ne 5-or $args[0]-cne '--config'-or $args[1]-cne $config-or $args[2]-cne 'client'-or $args[3]-cne 'application-state'-or $args[4]-cne 'status-video'){throw 'Unexpected video lifecycle CLI arguments.'}
 Write-Host '__LIFECYCLE_PROBE__'
 $global:LASTEXITCODE=if($testCase-eq 'lifecycle_error'){1}else{0}
 if($global:LASTEXITCODE-ne 0){return}
 [ordered]@{stopped=($testCase-eq 'manually_stopped')}|ConvertTo-Json -Compress
}
function Get-ScheduledTask {
 [CmdletBinding()]param($TaskPath,$TaskName)
 if($testCase-eq 'missing'){return $null}
 $owner=if($testCase-eq 'foreign'){'C:\Foreign\usbbridge-camera.exe'}else{$exe}
 $state=if($testCase-eq 'disabled'){'Disabled'}elseif($script:stopped){'Ready'}else{'Running'}
 [pscustomobject]@{State=$state;Actions=@([pscustomobject]@{Execute=$owner;Arguments=($testMode+' --config "'+$config+'"')})}
}
function Get-Content {
 [CmdletBinding()]param($LiteralPath,[switch]$Raw,$Encoding)
 if($LiteralPath-like '*.status.json'){
  $age=if($testCase-in @('hung','startup','manually_stopped','lifecycle_error')){180000}else{0}
  [ordered]@{running=$true;process_id=222;updated_unix_ms=([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-$age)}|ConvertTo-Json -Compress
 }else{[ordered]@{enabled=($testCase-ne 'settings_off')}|ConvertTo-Json -Compress}
}
function Get-CimInstance {
 [CmdletBinding()]param($ClassName,$Filter)
 if($testCase-in @('exited','manually_stopped','lifecycle_error')){return @()}
 $age=if($testCase-eq 'startup'){0}else{180}
 [pscustomobject]@{ProcessId=222;ExecutablePath=$exe;CommandLine=('"'+$exe+'" '+$testMode+' --config "'+$config+'"');CreationDate=[DateTime]::UtcNow.AddSeconds(-$age)}
}
function Get-Process {
 [CmdletBinding()]param($Id)
 [pscustomobject]@{Id=$Id;Handle=[IntPtr]::Zero}|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {param($ms) return $true} -PassThru
}
function Stop-Process {
 [CmdletBinding()]param($InputObject,[switch]$Force)
 Write-Output '__PROCESS_STOP__'
}
function Stop-ScheduledTask {
 [CmdletBinding()]param([Parameter(ValueFromPipeline=$true)]$InputObject)
 process{$script:stopped=$true;Write-Output '__TASK_STOP__'}
}
function Start-ScheduledTask {
 [CmdletBinding()]param([Parameter(ValueFromPipeline=$true)]$InputObject)
 process{Write-Output '__START__'}
}
& 'HELPER' -Role $testRole -Config $config
"""
            replacements={"ROLE":role,"CASE":case,"RUNTIME":str(ROOT),"CONFIG":str(Path(directory)/"receiver.local.json"),"HELPER":str(ROOT/"Restart-VideoEndpoint.ps1")}
            for key,value in replacements.items():script=script.replace(key,value.replace("'","''"))
            return subprocess.run(["powershell.exe","-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",script],capture_output=True,text=True,timeout=25)

    def test_disabled_missing_or_settings_off_is_not_resurrected(self):
        for case in ("disabled","missing","settings_off"):
            with self.subTest(case=case):
                result=self.run_helper(case)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertNotIn("__START__",result.stdout)
                self.assertNotIn("__PROCESS_STOP__",result.stdout)

    def test_foreign_task_is_refused_without_stopping_a_process(self):
        result=self.run_helper("foreign")
        self.assertNotEqual(result.returncode,0)
        self.assertNotIn("__START__",result.stdout)
        self.assertNotIn("__PROCESS_STOP__",result.stdout)

    def test_healthy_or_starting_endpoint_is_preserved(self):
        for case in ("healthy","startup"):
            with self.subTest(case=case):
                result=self.run_helper(case)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertNotIn("__START__",result.stdout)

    def test_exited_endpoint_is_restarted_despite_a_recent_status_file(self):
        result=self.run_helper("exited")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("restart_requested",result.stdout)
        self.assertIn("__START__",result.stdout)
        self.assertNotIn("__PROCESS_STOP__",result.stdout)

    def test_hung_owned_endpoint_exits_before_restart(self):
        result=self.run_helper("hung")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("restart_requested",result.stdout)
        self.assertLess(result.stdout.index("__PROCESS_STOP__"),result.stdout.index("__START__"))

    def test_local_sender_watchdog_preserves_healthy_sender(self):
        result=self.run_helper("healthy", "LocalSend")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertNotIn("__START__",result.stdout)

    def test_local_sender_watchdog_recovers_its_exited_sender(self):
        result=self.run_helper("exited", "LocalSend")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("__START__",result.stdout)

    def test_local_sender_watchdog_refuses_foreign_runtime(self):
        result=self.run_helper("foreign", "LocalSend")
        self.assertNotEqual(result.returncode,0)
        self.assertNotIn("__START__",result.stdout)
        self.assertNotIn("__PROCESS_STOP__",result.stdout)

    def test_explicitly_exited_role_does_not_restart_video(self):
        for role in ("Receive", "Send", "LocalSend"):
            with self.subTest(role=role):
                result=self.run_helper("manually_stopped", role)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn("__LIFECYCLE_PROBE__",result.stdout)
                self.assertIn("manually_stopped",result.stdout)
                self.assertNotIn("__START__",result.stdout)
                self.assertNotIn("__PROCESS_STOP__",result.stdout)

    def test_failed_lifecycle_probe_never_restarts_video(self):
        result=self.run_helper("lifecycle_error")
        self.assertNotEqual(result.returncode,0)
        self.assertIn("__LIFECYCLE_PROBE__",result.stdout)
        self.assertNotIn("__START__",result.stdout)
