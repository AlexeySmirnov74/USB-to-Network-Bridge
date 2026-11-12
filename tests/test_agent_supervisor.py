"""Task ownership and intentional-disable regression checks; all scheduler calls are mocked."""
import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipUnless(os.name=="nt","Windows scheduler helper")
class AgentSupervisorTests(unittest.TestCase):
    def run_helper(self, mode, role="Client"):
        helper=str(ROOT/"Restart-AgentSupervisor.ps1").replace("'","''")
        runtime=str(ROOT).replace("'","''")
        mode=mode.replace("'","''")
        script=r"""
$ErrorActionPreference='Stop'
$testCase='MODE'
$testRole='ROLE';$testRoleMode=if($testRole-eq 'Server'){'--host'}else{'--client'}
$script:taskStopped=$false
$runtime='RUNTIME'
$config='C:\USBBridgeFixture\client.local.json'
$agent=Join-Path $runtime 'usbbridge-agent.exe'
# Mock the fully qualified command, so no built EXE or live application state
# is needed in a clean checkout. Validate the helper's complete CLI contract.
Set-Item -LiteralPath ('Function:'+(Join-Path $runtime 'usbbridge.exe')) -Value {
 if($args.Count-ne 5-or $args[0]-cne '--config'-or $args[1]-cne $config-or $args[2]-cne $testRole.ToLowerInvariant()-or $args[3]-cne 'application-state'-or $args[4]-cne 'status'){throw 'Unexpected lifecycle CLI arguments.'}
 Write-Host '__LIFECYCLE_PROBE__'
 $global:LASTEXITCODE=if($testCase-eq 'lifecycle_error'){1}else{0}
 if($global:LASTEXITCODE-ne 0){return}
 [ordered]@{stopped=($testCase-eq 'manually_stopped')}|ConvertTo-Json -Compress
}
function Get-ScheduledTask {
 [CmdletBinding()]param($TaskPath,$TaskName)
 if($testCase-eq 'missing'){return $null}
 $state=if($testCase-eq 'disabled'){'Disabled'}elseif($script:taskStopped){'Ready'}else{'Running'}
 $owner=if($testCase-eq 'foreign'){'C:\Foreign\usbbridge-agent.exe'}else{$agent}
 [pscustomobject]@{State=$state;TaskName=('USBBridge.'+$testRole);Actions=@([pscustomobject]@{Execute=$owner;Arguments=('--supervise '+$testRoleMode+' --config "'+$config+'"')})}
}
function Get-Content {
 [CmdletBinding()]param($LiteralPath,[switch]$Raw)
 $pidValue=if($LiteralPath-like '*.supervisor.local.json'-or $testCase-eq 'wrong_controller_pid'){111}else{222}
 $age=if($testCase-in @('stale','manually_stopped','lifecycle_error')-or ($testCase-eq 'frozen_supervisor'-and $LiteralPath-like '*.supervisor.local.json')){180000}else{0}
 [ordered]@{process_id=$pidValue;role=$testRole.ToLowerInvariant();running=$true;updated_unix_ms=([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-$age)}|ConvertTo-Json -Compress
}
function Get-CimInstance {
 [CmdletBinding()]param($ClassName,$Filter)
 $arguments=if($Filter-eq 'ProcessId=111'){'--supervise '+$testRoleMode}else{$testRoleMode+' --supervised-by 111'}
 [pscustomobject]@{ProcessId=([uint32]($Filter-split '=')[1]);ExecutablePath=$agent;CommandLine=('"'+$agent+'" '+$arguments+' --config "'+$config+'"')}
}
function Get-Process {
 [CmdletBinding()]param($Id)
 [pscustomobject]@{Id=$Id;Handle=[IntPtr]::Zero}|Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {param($timeout) Write-Host '__EXIT_WAITED__'; return $true} -PassThru
}
function Stop-Process {
 [CmdletBinding()]param($InputObject,[switch]$Force)
 Write-Output ('__PROCESS_STOP_'+$InputObject.Id+'__')
}
function Stop-ScheduledTask {
 [CmdletBinding()]param([Parameter(ValueFromPipeline=$true)]$InputObject)
 process {$script:taskStopped=$true;Write-Output '__STOP__'}
}
function Start-ScheduledTask {
 [CmdletBinding()]param([Parameter(ValueFromPipeline=$true)]$InputObject)
 process {Write-Output '__START__'}
}
& 'HELPER' -Role $testRole -Config $config
"""
        script=script.replace("MODE",mode).replace("ROLE",role).replace("RUNTIME",runtime).replace("HELPER",helper)
        return subprocess.run(["powershell.exe","-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-Command",script],capture_output=True,text=True,timeout=20)

    def test_disabled_or_missing_role_is_never_resurrected(self):
        for mode in ("disabled","missing"):
            with self.subTest(mode=mode):
                result=self.run_helper(mode)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn("disabled_or_not_registered",result.stdout)
                self.assertNotIn("__START__",result.stdout)

    def test_foreign_task_is_refused_without_stopping_it(self):
        result=self.run_helper("foreign")
        self.assertNotEqual(result.returncode,0)
        self.assertNotIn("__STOP__",result.stdout)
        self.assertNotIn("__START__",result.stdout)

    def test_current_heartbeats_do_not_interrupt_a_working_role(self):
        result=self.run_helper("healthy")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("already_healthy",result.stdout)
        self.assertNotIn("__STOP__",result.stdout)

    def test_stale_role_is_restarted(self):
        result=self.run_helper("stale")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("__STOP__",result.stdout)
        self.assertIn("__START__",result.stdout)
        self.assertIn("restart_requested",result.stdout)

    def test_supervisor_pid_cannot_masquerade_as_the_controller(self):
        result=self.run_helper("wrong_controller_pid")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("restart_requested",result.stdout)

    def test_frozen_supervisor_exits_before_a_replacement_is_started(self):
        result=self.run_helper("frozen_supervisor")
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn("restart_requested",result.stdout)
        self.assertLess(result.stdout.index("__PROCESS_STOP_111__"),result.stdout.index("__EXIT_WAITED__"))
        self.assertLess(result.stdout.index("__EXIT_WAITED__"),result.stdout.index("__START__"))

    def test_explicitly_exited_role_is_not_restarted(self):
        for role in ("Client", "Server"):
            with self.subTest(role=role):
                result=self.run_helper("manually_stopped", role)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn("__LIFECYCLE_PROBE__",result.stdout)
                self.assertIn("manually_stopped",result.stdout)
                self.assertNotIn("__START__",result.stdout)

    def test_failed_lifecycle_probe_never_restarts_the_role(self):
        result=self.run_helper("lifecycle_error")
        self.assertNotEqual(result.returncode,0)
        self.assertIn("__LIFECYCLE_PROBE__",result.stdout)
        self.assertNotIn("__START__",result.stdout)
