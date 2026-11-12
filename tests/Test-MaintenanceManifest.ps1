param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory 'Maintain-Installation.ps1'),[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Maintenance script did not parse.'}
foreach($name in @('Get-MaintenanceConfigArgument','Get-MaintenanceRoleConfig','Update-MaintenanceDeploymentManifest')){
 $node=$ast.Find({param($item)$item-is [Management.Automation.Language.FunctionDefinitionAst]-and $item.Name-eq $name},$true)
 if(-not $node){throw ('Missing maintenance function: '+$name)}
 . ([scriptblock]::Create($node.Extent.Text))
}
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge manifest fixture '+[guid]::NewGuid().ToString('N'))
$runtime=Join-Path $testRoot 'app'
function Assert-ManifestFixture($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Manifest refresh assertion failed: '+$Name)}}
try {
 New-Item -ItemType Directory -Path $runtime -Force|Out-Null
 [IO.File]::WriteAllText((Join-Path $runtime 'deployment.local.json'),'fixture-stamp')
 $config=Join-Path $runtime 'host.local.json';[IO.File]::WriteAllText($config,'fixture-settings-unchanged')
 $before=(Get-FileHash -LiteralPath $config).Hash
 $stub=@'
function Get-BridgeDeploymentBinding {param($Runtime) $global:MaintenanceManifestFixture.binding}
function Test-BridgeSamePath {param($First,$Second) [string]::Equals([IO.Path]::GetFullPath($First),[IO.Path]::GetFullPath($Second),[StringComparison]::OrdinalIgnoreCase)}
function Get-ScheduledTask {param($TaskPath,$ErrorAction) $global:MaintenanceManifestFixture.task_reads++; $global:MaintenanceManifestFixture.tasks}
function Get-BridgeMachineMarker {param($Role,$Runtime) $global:MaintenanceManifestFixture.markers[$Role]}
function Get-BridgeMachineIdentity {'fixture-machine'}
function Register-BridgeDeploymentBinding {param($Runtime,$InstallRoot,$RoleConfigs) $global:MaintenanceManifestFixture.registrations.Add([pscustomobject]@{runtime=$Runtime;root=$InstallRoot;roles=$RoleConfigs})}
'@
 [IO.File]::WriteAllText((Join-Path $runtime 'FirstRun-Identity.ps1'),$stub,[Text.UTF8Encoding]::new($true))
 $agent=Join-Path $runtime 'usbbridge-agent.exe'
 $global:MaintenanceManifestFixture=@{
  binding=[pscustomobject]@{RuntimeDirectory=$runtime;DeploymentId='fixture-deployment'}
  tasks=@([pscustomobject]@{TaskName='USBBridge.Server';Actions=@([pscustomobject]@{Execute=$agent;Arguments=('--supervise --host --config "'+$config+'"')})})
  markers=@{Server=[pscustomobject]@{schema=2;initialized=$true;machine_id='fixture-machine';deployment_id='fixture-deployment';config=$config}}
  task_reads=0;registrations=(New-Object 'Collections.Generic.List[object]')
 }
 Update-MaintenanceDeploymentManifest $runtime
 Assert-ManifestFixture $global:MaintenanceManifestFixture.registrations.Count 1 'verified same-machine role refreshes binding'
 Assert-ManifestFixture $global:MaintenanceManifestFixture.registrations[0].roles['Server'] $config 'registered role preserves config'
 Assert-ManifestFixture $global:MaintenanceManifestFixture.registrations[0].root $testRoot 'install root is app parent'
 Assert-ManifestFixture (Get-FileHash -LiteralPath $config).Hash $before 'config bytes unchanged'
 $global:MaintenanceManifestFixture.registrations.Clear();$global:MaintenanceManifestFixture.task_reads=0
 $global:MaintenanceManifestFixture.binding=[pscustomobject]@{RuntimeDirectory=(Join-Path $testRoot 'other-app');DeploymentId='fixture-deployment'}
 Update-MaintenanceDeploymentManifest $runtime
 Assert-ManifestFixture $global:MaintenanceManifestFixture.registrations.Count 0 'another runtime binding untouched'
 Assert-ManifestFixture $global:MaintenanceManifestFixture.task_reads 0 'another runtime exits before task inspection'
 $global:MaintenanceManifestFixture.binding=[pscustomobject]@{RuntimeDirectory=$runtime;DeploymentId='fixture-deployment'}
 $global:MaintenanceManifestFixture.markers.Server.machine_id='foreign-machine'
 Update-MaintenanceDeploymentManifest $runtime
 Assert-ManifestFixture $global:MaintenanceManifestFixture.registrations.Count 0 'foreign marker never refreshes binding'
 Assert-ManifestFixture (Get-FileHash -LiteralPath $config).Hash $before 'negative cases preserve config'
 [pscustomobject]@{passed=$true;cases=3;assertions=8;scope='maintenance payload registration with mocked tasks and registry'}|ConvertTo-Json -Compress
}finally{
 Remove-Variable -Name MaintenanceManifestFixture -Scope Global -ErrorAction SilentlyContinue
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
