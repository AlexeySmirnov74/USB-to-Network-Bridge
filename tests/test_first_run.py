"""Focused Windows first-run identity/reset policy; no drivers or live tasks."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.name == 'nt', 'Windows PowerShell policy helper')
class FirstRunPolicyTests(unittest.TestCase):
    def run_policy(self, body):
        with tempfile.TemporaryDirectory(prefix='usbbridge-first-run-') as directory:
            source = ROOT / 'FirstRun-Identity.ps1'
            code = "$ErrorActionPreference='Stop'; . '" + str(source).replace("'", "''") + "'; $testRoot='" + directory.replace("'", "''") + "'; " + body
            result = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', code],
                                    capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return json.loads(result.stdout.lstrip('\ufeff'))

    def test_hashed_identity_is_stable_case_insensitive_and_not_raw_guid(self):
        value = self.run_policy("$a=ConvertTo-BridgeMachineIdentity 'A067E6F6-E90A-4078-A66A-27E9CF8D8438'; $b=ConvertTo-BridgeMachineIdentity 'a067e6f6-e90a-4078-a66a-27e9cf8d8438'; @{same=($a-eq $b);value=$a}|ConvertTo-Json")
        self.assertTrue(value['same'])
        self.assertRegex(value['value'], r'^[a-f0-9]{64}$')
        self.assertNotIn('a067e6f6', value['value'])

    def test_marker_without_hash_never_trusts_copied_initialized_state(self):
        value = self.run_policy("$config=Join-Path $testRoot 'host.local.json'; Write-BridgeJson $config @{version=1}; Write-BridgeJson (Get-BridgeMachineMarkerPath Server $testRoot) @{initialized=$true;runtime=$testRoot}; @{unknown=(Get-BridgeMachineDisposition -Role Server -Runtime $testRoot -Config $config -MachineIdentity ('a'*64));owned=(Get-BridgeMachineDisposition -Role Server -Runtime $testRoot -Config $config -MachineIdentity ('a'*64) -OwnedTask)}|ConvertTo-Json")
        self.assertEqual(value, {'unknown': 'Unverified', 'owned': 'Adopt'})

    def test_same_machine_marker_and_foreign_marker_are_distinct(self):
        value = self.run_policy("$config=Join-Path $testRoot 'client.local.json'; Write-BridgeJson $config @{version=1}; Write-BridgeJson (Get-BridgeMachineMarkerPath Client $testRoot) @{schema=1;role='Client';initialized=$true;machine_id=('a'*64)}; @{same=(Get-BridgeMachineDisposition -Role Client -Runtime $testRoot -Config $config -MachineIdentity ('a'*64));foreign=(Get-BridgeMachineDisposition -Role Client -Runtime $testRoot -Config $config -MachineIdentity ('b'*64) -OwnedTask)}|ConvertTo-Json")
        self.assertEqual(value, {'same': 'Current', 'foreign': 'Foreign'})

    def test_new_server_backs_up_and_clears_publications_camera_and_identity(self):
        value = self.run_policy("$config=Join-Path $testRoot 'host.local.json'; Write-BridgeJson $config @{enabled=$true;server_id='old';name='OLDPC'}; Write-BridgeJson (Join-Path $testRoot 'host.video-receive.local.json') @{key_file=(Join-Path $testRoot 'camera.key');enabled=$true}; [IO.File]::WriteAllText((Join-Path $testRoot 'camera.key'),'fixture-not-secret'); $backup=Reset-BridgeRoleState -Role Server -Runtime $testRoot -Config $config -ComputerName NEWPC; $settings=Read-BridgeJson $config; $backend=Read-BridgeJson (Join-Path $testRoot 'backend-state.json'); @{fresh=($settings.server_id-ne 'old');enabled=$settings.enabled;name=$settings.name;allowed=@($settings.allowed_ips);cameraExists=(Test-Path -LiteralPath (Join-Path $testRoot 'host.video-receive.local.json'));keyExists=(Test-Path -LiteralPath (Join-Path $testRoot 'camera.key'));backup=(Test-Path -LiteralPath $backup);reservations=@($backend.Reservations);forced=@($backend.ForcedByUs);exports=@((Import-PowerShellDataFile -LiteralPath (Join-Path $testRoot 'server.local.psd1')).Export)}|ConvertTo-Json -Depth 4")
        self.assertTrue(value['fresh'] and value['backup'])
        self.assertFalse(value['enabled'] or value['cameraExists'] or value['keyExists'])
        self.assertEqual(value['name'], 'NEWPC')
        self.assertEqual(value['allowed'], ['127.0.0.1'])
        self.assertEqual(value['reservations'], [])
        self.assertEqual(value['forced'], [])
        self.assertEqual(value['exports'], [])

    def test_keep_client_retains_selections_but_strips_machine_bound_auth_ports(self):
        value = self.run_policy("$settings=@{version=1;server='192.0.2.10:3240';devices=@(@{identity='USB\\VID_1234';vendor=4660;product=1;busid='2-3';enabled=$true});proxy=@{kind='socks5';address='proxy.invalid:1080';credential_file='C:\\OLDPC\\private.bin'};servers=@(@{server='192.0.2.11:3240';devices=@();proxy=@{kind='http';address='proxy.invalid:8080';credential_file='C:\\OLDPC\\other.bin'}})}; $copy=ConvertTo-BridgePortableClientSettings $settings; @{server=$copy.server;identity=$copy.devices[0].identity;enabled=$copy.devices[0].enabled;busid=$copy.devices[0].busid;auth=$copy.proxy.credential_file;other=$copy.servers[0].server;otherAuth=$copy.servers[0].proxy.credential_file;original=$settings.proxy.credential_file}|ConvertTo-Json")
        self.assertEqual(value['server'], '192.0.2.10:3240')
        self.assertEqual(value['other'], '192.0.2.11:3240')
        self.assertTrue(value['enabled'])
        self.assertEqual(value['busid'], '')
        self.assertIsNone(value['auth'])
        self.assertIsNone(value['otherAuth'])
        self.assertIsNotNone(value['original'])

    def test_driver_service_record_alone_does_not_mean_ready(self):
        value = self.run_policy("$script:driverMode='Manual'; function Get-CimInstance {param($ClassName,$Filter,$ErrorAction) [pscustomobject]@{PathName=(Join-Path $testRoot 'driver.sys');StartMode=$script:driverMode}}; function Get-Service {param($Name,$ErrorAction) [pscustomobject]@{Name='usbipd'}}; $missing=Get-BridgeDriverReadiness Server; [IO.File]::WriteAllText((Join-Path $testRoot 'driver.sys'),'fixture'); $ready=Get-BridgeDriverReadiness Server; $script:driverMode='Disabled'; $disabled=Get-BridgeDriverReadiness Server; @{missing=$missing.Ready;ready=$ready.Ready;disabled=$disabled.Ready}|ConvertTo-Json")
        self.assertEqual(value, {'missing': False, 'ready': True, 'disabled': False})

    def test_foreign_server_moves_publications_inventory_history_not_client(self):
        value = self.run_policy("$config=Join-Path $testRoot 'host.local.json'; Write-BridgeJson $config @{enabled=$true}; $names=@('native-publication.local.json','native-sessions.local.json','native-hid-guards.local.json','device-locations.local.json','devices.inventory','host.local.devices.local.json','host.local.catalog.local.json','host.local.preferences.json'); foreach($name in $names){[IO.File]::WriteAllText((Join-Path $testRoot $name),'fixture')}; [IO.File]::WriteAllText((Join-Path $testRoot 'client.local.json'),'other-role'); $backup=Reset-BridgeRoleState -Role Server -Runtime $testRoot -Config $config -ComputerName NEWPC; @{remaining=@($names|Where-Object {Test-Path -LiteralPath (Join-Path $testRoot $_)});backupCount=@(Get-ChildItem -LiteralPath $backup -File).Count;client=[IO.File]::ReadAllText((Join-Path $testRoot 'client.local.json'))}|ConvertTo-Json")
        self.assertEqual(value['remaining'], [])
        self.assertGreaterEqual(value['backupCount'], 9)
        self.assertEqual(value['client'], 'other-role')

    def test_client_keep_preserves_language_but_clears_old_ui_routes_camera_state(self):
        value = self.run_policy("$config=Join-Path $testRoot 'client.local.json'; Write-BridgeJson $config @{version=1;server='192.0.2.10:3240';devices=@();servers=@()}; Write-BridgeJson ([IO.Path]::ChangeExtension($config,'preferences.json')) @{language='de'}; foreach($extension in @('proxy-routes.local.json','status.json','catalog.local.json','camera.preferences.json','health.local.json','supervisor.local.json')){Write-BridgeJson ([IO.Path]::ChangeExtension($config,$extension)) @{fixture='stale'}}; $null=Reset-BridgeRoleState -Role Client -Runtime $testRoot -Config $config -KeepClientConnections; @{language=(Read-BridgeJson ([IO.Path]::ChangeExtension($config,'preferences.json'))).language;server=(Read-BridgeJson $config).server;stale=@(@('proxy-routes.local.json','status.json','catalog.local.json','camera.preferences.json','health.local.json','supervisor.local.json')|Where-Object {Test-Path -LiteralPath ([IO.Path]::ChangeExtension($config,$_))})}|ConvertTo-Json")
        self.assertEqual(value, {'language': 'de', 'server': '192.0.2.10:3240', 'stale': []})

    def test_ancestor_acl_allows_standard_root_create_dirs_not_path_replacement(self):
        value = self.run_policy("$sid=New-Object Security.Principal.SecurityIdentifier('S-1-5-11'); $create=New-Object Security.AccessControl.FileSystemAccessRule($sid,'CreateDirectories','ContainerInherit','None','Allow'); $inherit=New-Object Security.AccessControl.FileSystemAccessRule($sid,'Modify','ContainerInherit','InheritOnly','Allow'); $delete=New-Object Security.AccessControl.FileSystemAccessRule($sid,'DeleteSubdirectoriesAndFiles','ContainerInherit','None','Allow'); $admin=New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')),'FullControl','ContainerInherit','None','Allow'); @{standard=(Test-BridgeBroadReplacementPermission @($create,$inherit,$admin));unsafe=(Test-BridgeBroadReplacementPermission @($delete))}|ConvertTo-Json")
        self.assertEqual(value, {'standard': False, 'unsafe': True})

    def test_role_secret_cleanup_handles_copied_nested_path_and_shared_other_role(self):
        value = self.run_policy("$config=Join-Path $testRoot 'host.local.json'; Write-BridgeJson $config @{enabled=$true}; $keys=Join-Path $testRoot 'video-credentials'; New-Item -ItemType Directory -Path $keys|Out-Null; [IO.File]::WriteAllText((Join-Path $keys 'owned.credential'),'fixture'); [IO.File]::WriteAllText((Join-Path $keys 'shared.key'),'fixture'); Write-BridgeJson (Join-Path $testRoot 'host.video-receive.local.json') @{key_file='C:\\OLDPC\\video-credentials\\owned.credential'}; Write-BridgeJson (Join-Path $testRoot 'host.video-send.local.json') @{key_file='C:\\OLDPC\\video-credentials\\shared.key'}; Write-BridgeJson (Join-Path $testRoot 'client.video-send.local.json') @{key_file='C:\\OLDPC\\video-credentials\\shared.key'}; $null=Reset-BridgeRoleState -Role Server -Runtime $testRoot -Config $config -ComputerName NEWPC; @{owned=(Test-Path -LiteralPath (Join-Path $keys 'owned.credential'));shared=(Test-Path -LiteralPath (Join-Path $keys 'shared.key'));otherRole=(Test-Path -LiteralPath (Join-Path $testRoot 'client.video-send.local.json'))}|ConvertTo-Json")
        self.assertEqual(value, {'owned': False, 'shared': True, 'otherRole': True})


if __name__ == '__main__':
    unittest.main()
