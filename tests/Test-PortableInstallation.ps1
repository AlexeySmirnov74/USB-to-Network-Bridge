param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'FirstRun-Identity.ps1')
$script:machine='a'*64
function Get-BridgeMachineIdentity {$script:machine}
$script:registry=@{}
function Get-ItemProperty {param($LiteralPath,$ErrorAction) if($script:registry.ContainsKey($LiteralPath)){[pscustomobject]$script:registry[$LiteralPath]}}
function New-Item {
 param($Path,$ItemType,[switch]$Force)
 if($Path-like 'HKLM:\*'){if(-not $script:registry.ContainsKey($Path)){$script:registry[$Path]=[ordered]@{}}}
 else{Microsoft.PowerShell.Management\New-Item @PSBoundParameters}
}
function New-ItemProperty {param($LiteralPath,$Name,$Value,$PropertyType,[switch]$Force) $script:registry[$LiteralPath][$Name]=$Value}
function Remove-ItemProperty {param($LiteralPath,$Name,$ErrorAction) if($script:registry.ContainsKey($LiteralPath)){$script:registry[$LiteralPath].Remove($Name)}}
function Set-Acl {param($LiteralPath,$AclObject) if($LiteralPath-notlike 'HKLM:\*'){Microsoft.PowerShell.Security\Set-Acl @PSBoundParameters}}
$script:checks=0
function Assert-Equal($Actual,$Expected,[string]$Label){if($Actual-cne $Expected){throw ('Installation fixture failed: '+$Label)};$script:checks++}
function Assert-Throws([scriptblock]$Action,[string]$Label){$failed=$false;try{&$Action}catch{$failed=$true};if(-not $failed){throw ('Expected failure: '+$Label)};$script:checks++}
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('bridge-installation-'+[guid]::NewGuid().ToString('N'))
$required=@('USB-to-Network-Bridge-Client.exe','USB-to-Network-Bridge-Server.exe','app\usbbridge.exe','app\usbbridge-client.exe','app\usbbridge-server.exe','app\usbbridge-agent.exe','app\usbbridge-camera.exe','app\Open-Installation.ps1','app\Configure-Installation.ps1','app\FirstRun-Identity.ps1','app\Portable-Paths.ps1','app\Portable-Installation.ps1','app\Relocate-Installation.ps1','app\Maintain-Installation.ps1','app\New-BackendMutex.ps1','app\Update-PortableSettings.ps1','app\Set-Autostart.ps1','app\Switch-Backend.ps1','app\Configure-Server.ps1','app\Publish-Device.ps1','app\Set-ServerState.ps1','app\Set-VideoMode.ps1','app\Restart-VideoEndpoint.ps1','app\Ensure-Exports.ps1','app\Verify-DriverInstaller.ps1','app\Ensure-ProductionDrivers.ps1','app\Install-ProductionDrivers.ps1','app\Download-Drivers.ps1','app\driver-installers.json','app\tools\driver-installers\USBip-0.9.8.1-x64.exe','app\tools\driver-installers\usbipd-win_5.3.0_x64.msi')
function New-Payload([string]$Root) {
 $records=@()
 foreach($name in $required){
  $path=Join-Path $Root $name
  New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force|Out-Null
  [IO.File]::WriteAllText($path,('fixture '+$name),[Text.UTF8Encoding]::new($false))
  $records+=@([pscustomobject]@{File=$name.Replace('\','/');Bytes=(Get-Item -LiteralPath $path).Length;SHA256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash})
 }
 Write-BridgeInstallationJson (Join-Path $Root 'app\SHA256.json') $records
}
function Pin-Payload([string]$Runtime,$Binding){$Binding.PayloadManifestSHA256=(Get-FileHash -LiteralPath (Join-Path $Runtime 'SHA256.json') -Algorithm SHA256).Hash}
try {
 $root=Join-Path $testRoot 'Исходная папка';$runtime=Join-Path $root 'app'
 New-Payload $root
 $serverConfig=Join-Path $runtime 'data\Server\host.local.json'
 $clientConfig=Join-Path $runtime 'data\Client\client.local.json'
 Write-BridgeInstallationJson $serverConfig @{server_id='unchanged-server';runtime='@app';profile='server.local.psd1'}
 Write-BridgeInstallationJson $clientConfig @{server='192.0.2.1:3240'}
 $null=Write-BridgeMachineMarker -Role Server -Runtime $runtime -Config $serverConfig -InstallRoot $root -AdoptExisting
 $null=Write-BridgeMachineMarker -Role Client -Runtime $runtime -Config $clientConfig -InstallRoot $root
 $rawServer=Read-BridgeInstallationJson (Get-BridgeMachineMarkerPath Server $runtime)
 $rawClient=Read-BridgeInstallationJson (Get-BridgeMachineMarkerPath Client $runtime)
 Assert-Equal $rawServer.schema 2 'schema2'
 Assert-Equal $rawServer.paths_version 1 'portable path version'
 Assert-Equal $rawServer.runtime '@app' 'runtime locator'
 Assert-Equal $rawServer.install_root '..' 'root locator'
 Assert-Equal $rawServer.config '@app/data/Server/host.local.json' 'role config locator'
 Assert-Equal $rawServer.deployment_id $rawClient.deployment_id 'shared deployment id'
 $resolved=Get-BridgeMachineMarker -Role Server -Runtime $runtime
 Assert-Equal $resolved.config $serverConfig 'config resolved absolute'
 Assert-Equal $resolved.runtime $runtime 'runtime resolved absolute'
 Assert-Equal $resolved.install_root $root 'root resolved absolute'
 Assert-Equal (Get-BridgeMachineDisposition -Role Server -Runtime $runtime -Config $serverConfig -MachineIdentity $script:machine) 'Current' 'same machine disposition'
 $rawServer.paths_version=0;Write-BridgeInstallationJson (Get-BridgeMachineMarkerPath Server $runtime) $rawServer
 Assert-Equal (Get-BridgeMachineMarker -Role Server -Runtime $runtime).paths_version 0 'migration bootstrap marker accepted'
 $rawServer.paths_version=1;Write-BridgeInstallationJson (Get-BridgeMachineMarkerPath Server $runtime) $rawServer
 $binding=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $root -RoleConfigs @{Server=$serverConfig}
 $binding=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $root -RoleConfigs @{Client=$clientConfig}
 Assert-Equal $binding.ServerConfig '@app/data/Server/host.local.json' 'registration merges previous role'
 Assert-Equal $binding.ClientConfig '@app/data/Client/client.local.json' 'registration new role'
 Assert-Equal (Get-BridgeDeploymentBinding -Runtime $runtime).MachineId $script:machine 'binding current machine'
 Assert-BridgeRelocatedRuntime -InstallRoot $root -Runtime $runtime -Binding $binding;$script:checks++
 $external=Join-Path $testRoot 'external\client.json';Write-BridgeInstallationJson $external @{version=1}
 $null=Write-BridgeMachineMarker -Role Client -Runtime $runtime -Config $external -InstallRoot $root
 $binding=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $root -RoleConfigs @{Client=$external}
 Assert-Equal $binding.ClientConfig '@external' 'external locator'
 Assert-Equal (Get-BridgeMachineMarker -Role Client -Runtime $runtime).config $external 'external config from protected registration'
 $script:machine='b'*64
 Assert-Equal (Get-BridgeDeploymentBinding -Runtime $runtime) $null 'foreign binding ignored'
 $foreign=Get-BridgeMachineMarker -Role Client -Runtime $runtime
 Assert-Equal $foreign.config $null 'foreign external locator unresolved'
 Assert-Equal $foreign.unresolved_external $true 'foreign external flag'
 Assert-Equal (Get-BridgeMachineDisposition -Role Client -Runtime $runtime -Config $clientConfig -MachineIdentity $script:machine) 'Foreign' 'foreign machine disposition'
 $script:machine='a'*64
 $null=Write-BridgeMachineMarker -Role Client -Runtime $runtime -Config $clientConfig -InstallRoot $root
 $binding=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $root -RoleConfigs @{Client=$clientConfig}
 Assert-Equal ($binding.PSObject.Properties.Name-contains 'ClientExternalConfig') $false 'obsolete external registration removed'
 $key=Join-Path (Split-Path -Parent $clientConfig) 'video-credentials\preserved.key'
 New-Item -ItemType Directory -Path (Split-Path -Parent $key) -Force|Out-Null
 [IO.File]::WriteAllText($key,'unchanged fixture bytes')
 $keyHash=(Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash
 $newRoot=Join-Path $testRoot 'Перенесённая папка с пробелами';Move-Item -LiteralPath $root -Destination $newRoot
 $newRuntime=Join-Path $newRoot 'app'
 $moved=Get-BridgeMachineMarker -Role Client -Runtime $newRuntime
 Assert-Equal $moved.config (Join-Path $newRuntime 'data\Client\client.local.json') 'physical folder move rebases config'
 Assert-Equal (Get-BridgeDeploymentBinding -Runtime $newRuntime).RuntimeDirectory $runtime 'binding retains old location for relocation'
 Assert-Equal (Get-FileHash -LiteralPath (Join-Path $newRuntime 'data\Client\video-credentials\preserved.key') -Algorithm SHA256).Hash $keyHash 'key bytes unchanged'
 Assert-Equal (Read-BridgeInstallationJson (Join-Path $newRuntime 'data\Server\host.local.json')).server_id 'unchanged-server' 'server identity unchanged'
 Assert-BridgeRelocatedRuntime -InstallRoot $newRoot -Runtime $newRuntime -Binding $binding;$script:checks++
 $tampered=Join-Path $newRuntime 'usbbridge-agent.exe';$bytes=[IO.File]::ReadAllBytes($tampered)
 [IO.File]::WriteAllText($tampered,'tampered');Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} 'payload modification rejected';[IO.File]::WriteAllBytes($tampered,$bytes)
 $manifestPath=Join-Path $newRuntime 'SHA256.json';$manifestBytes=[IO.File]::ReadAllBytes($manifestPath)
 [IO.File]::WriteAllText($manifestPath,'[]');Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} 'unpinned manifest rejected';[IO.File]::WriteAllBytes($manifestPath,$manifestBytes)
 $records=Read-BridgeInstallationJson $manifestPath
 Write-BridgeInstallationJson $manifestPath @($records|Where-Object {$_.File-ne 'app/Update-PortableSettings.ps1'});Pin-Payload $newRuntime $binding
 Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} 'required updater cannot be omitted'
 foreach($offlineRequired in @('app/Verify-DriverInstaller.ps1','app/Ensure-ProductionDrivers.ps1','app/Install-ProductionDrivers.ps1','app/Download-Drivers.ps1','app/driver-installers.json','app/tools/driver-installers/USBip-0.9.8.1-x64.exe','app/tools/driver-installers/usbipd-win_5.3.0_x64.msi')){
  Write-BridgeInstallationJson $manifestPath @($records|Where-Object {$_.File-ne $offlineRequired});Pin-Payload $newRuntime $binding
  Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} ('offline prerequisite cannot be omitted: '+$offlineRequired)
 }
 Write-BridgeInstallationJson $manifestPath @($records+$records[0]);Pin-Payload $newRuntime $binding
 Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} 'duplicate manifest entry rejected'
 $records[0].File='../outside.exe';Write-BridgeInstallationJson $manifestPath $records;Pin-Payload $newRuntime $binding
 Assert-Throws {Assert-BridgeRelocatedRuntime $newRoot $newRuntime $binding} 'manifest traversal rejected'
 [IO.File]::WriteAllBytes($manifestPath,$manifestBytes)
 $legacy=Join-Path $testRoot 'legacy\app';$legacyConfig=Join-Path $legacy 'data\Client\client.local.json';Write-BridgeInstallationJson $legacyConfig @{version=1}
 Write-BridgeInstallationJson (Get-BridgeMachineMarkerPath Client $legacy) @{schema=1;role='Client';initialized=$true;machine_id=$script:machine;runtime='C:\Old Location\app';config='C:\Old Location\app\data\Client\client.local.json';install_root='C:\Old Location'}
 $legacyMarker=Get-BridgeMachineMarker -Role Client -Runtime $legacy
 Assert-Equal $legacyMarker.config $legacyConfig 'schema1 moved config rebased'
 Assert-Equal $legacyMarker.old_runtime 'C:\Old Location\app' 'schema1 old runtime preserved in memory'
 Assert-Equal (Get-BridgeMachineDisposition -Role Client -Runtime $legacy -Config $legacyConfig -MachineIdentity $script:machine) 'Current' 'schema1 same machine move retains state'
 [pscustomobject]@{checks=$script:checks;success=$true;registry_writes=0;tasks_changed=0;drivers_installed=0}|ConvertTo-Json
}finally{if(Test-Path -LiteralPath $testRoot){Remove-Item -LiteralPath $testRoot -Recurse -Force}}
