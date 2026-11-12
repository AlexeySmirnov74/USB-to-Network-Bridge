# Dot-source only. Portable markers contain locators; Windows owns absolute registration paths.
. (Join-Path $PSScriptRoot 'Portable-Paths.ps1')

function Read-BridgeInstallationJson([string]$Path) {
 if(-not(Test-Path -LiteralPath $Path -PathType Leaf)){return $null}
 if((Get-Item -LiteralPath $Path).Length-gt 1048576){throw 'An installation settings file is too large.'}
 Get-Content -LiteralPath $Path -Raw -Encoding UTF8|ConvertFrom-Json
}
function Write-BridgeInstallationJson([string]$Path,$Value) {
 $directory=[IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Path))
 New-Item -ItemType Directory -Path $directory -Force|Out-Null
 $temporary=Join-Path $directory ('.installation-'+[guid]::NewGuid().ToString('N')+'.tmp')
 try {
  [IO.File]::WriteAllText($temporary,($Value|ConvertTo-Json -Depth 24),[Text.UTF8Encoding]::new($false))
  if(Test-Path -LiteralPath $Path){[IO.File]::Replace($temporary,$Path,[NullString]::Value)}else{[IO.File]::Move($temporary,$Path)}
 }finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Get-BridgeInstallationMachineIdentity {
 if(Get-Command Get-BridgeMachineIdentity -ErrorAction SilentlyContinue){return Get-BridgeMachineIdentity}
 $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine,[Microsoft.Win32.RegistryView]::Registry64)
 try {
  $key=$base.OpenSubKey('SOFTWARE\Microsoft\Cryptography',$false)
  if(-not $key){throw 'Windows machine identity is unavailable.'}
  try{$guid=[guid]::Parse([string]$key.GetValue('MachineGuid',''))}finally{$key.Dispose()}
 }finally{$base.Dispose()}
 $sha=[Security.Cryptography.SHA256]::Create()
 try{-join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(('USBtoNetworkBridge/machine/v1:'+$guid.ToString('D').ToLowerInvariant())))|ForEach-Object {$_.ToString('x2')})}finally{$sha.Dispose()}
}
function Get-BridgeDeploymentId([string]$Runtime,[switch]$Create) {
 $path=Join-Path $Runtime 'deployment.local.json'
 $stamp=Read-BridgeInstallationJson $path
 if($stamp){
  if($stamp.schema-ne 1-or -not $stamp.deployment_id){throw 'The deployment marker is invalid.'}
  return ([guid]::Parse([string]$stamp.deployment_id)).ToString('D').ToLowerInvariant()
 }
 if(-not $Create){return $null}
 $identifier=[guid]::NewGuid().ToString('D').ToLowerInvariant()
 Write-BridgeInstallationJson $path ([ordered]@{schema=1;deployment_id=$identifier;paths_version=1})
 $identifier
}
function Get-BridgeDeploymentBinding {
 param([string]$Runtime)
 $identifier=Get-BridgeDeploymentId $Runtime
 if(-not $identifier){return $null}
 $binding=Get-ItemProperty -LiteralPath ('HKLM:\Software\USBtoNetworkBridge\Deployments\'+$identifier) -ErrorAction SilentlyContinue
 if(-not $binding-or [string]$binding.MachineId-ne (Get-BridgeInstallationMachineIdentity)){return $null}
 $binding
}
function Test-BridgeInstalledRoleMarker {
 param($Marker,[string]$Role,[string]$Runtime,[string]$InstallRoot,[string]$Config,[string]$MarkerPath)
 if(-not $Marker-or -not $Marker.initialized-or -not $Marker.runtime){return $false}
 if($Marker.role-and $Marker.role-ne $Role){return $false}
 if($Marker.machine_id-and $Marker.machine_id-ne (Get-BridgeInstallationMachineIdentity)){return $false}
 try{
  $located=Resolve-BridgeConfigPath ([string]$Marker.runtime) $MarkerPath $Runtime
  if(-not(Test-BridgeSamePath $located $Runtime)-and -not(Test-BridgeSamePath $located $InstallRoot)){return $false}
  if($Marker.config){
   $locator=[string]$Marker.config
   if($locator-eq '@external'){$binding=Get-BridgeDeploymentBinding $Runtime;$selected=if($binding){[string]$binding.($Role+'ExternalConfig')}else{''}}
   else{$selected=Resolve-BridgeConfigPath $locator $MarkerPath $Runtime}
   if(-not $selected-or -not(Test-BridgeSamePath $selected $Config)){return $false}
  }
  return $true
 }catch{return $false}
}
function Get-BridgeRoleConfigLocator([string]$Config,[string]$Runtime) {
 $config=[IO.Path]::GetFullPath($Config)
 $runtime=[IO.Path]::GetFullPath($Runtime)
 if(Test-BridgePathWithin $config $runtime){return ('@app/'+(Get-BridgeRelativePath $config $runtime).Replace('\','/'))}
 $common=[Environment]::GetFolderPath('CommonApplicationData')
 if(Test-BridgePathWithin $config $common){return ('@programdata/'+(Get-BridgeRelativePath $config $common).Replace('\','/'))}
 '@external'
}
function Write-BridgePortableMachineMarker {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Runtime,[string]$Config,[string]$InstallRoot,[switch]$AdoptExisting)
 $runtime=[IO.Path]::GetFullPath($Runtime)
 if(-not $InstallRoot){$InstallRoot=if((Split-Path -Leaf $runtime)-eq 'app'){Split-Path -Parent $runtime}else{$runtime}}
 $path=Join-Path $runtime ('machine-'+$Role.ToLowerInvariant()+'.local.json')
 $root=Get-BridgeRelativePath ([IO.Path]::GetFullPath($InstallRoot)) $runtime
 if($null-eq $root){throw 'The installation root and runtime must share one volume.'}
 $state=[ordered]@{schema=2;paths_version=1;machine_id=(Get-BridgeInstallationMachineIdentity);deployment_id=(Get-BridgeDeploymentId $runtime -Create);role=$Role;initialized=$true;runtime='@app';config=(Get-BridgeRoleConfigLocator $Config $runtime);install_root=$root;adopted_existing=[bool]$AdoptExisting}
 Write-BridgeInstallationJson $path $state
 [pscustomobject]$state
}
function Get-BridgeMachineMarker {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Runtime)
 $runtime=[IO.Path]::GetFullPath($Runtime)
 $path=Join-Path $runtime ('machine-'+$Role.ToLowerInvariant()+'.local.json')
 $marker=Read-BridgeInstallationJson $path
 if(-not $marker){return $null}
 if($marker.schema-notin @(1,2)){return $marker}
 $oldRuntime=if($marker.schema-eq 1-and $marker.runtime){[IO.Path]::GetFullPath([string]$marker.runtime)}else{$null}
 $marker|Add-Member -NotePropertyName old_runtime -NotePropertyValue $oldRuntime -Force
 $marker|Add-Member -NotePropertyName runtime -NotePropertyValue $runtime -Force
 if($marker.schema-eq 1){
  if($marker.config){
   $config=[IO.Path]::GetFullPath([string]$marker.config)
   if($oldRuntime-and (Test-BridgePathWithin $config $oldRuntime)){$config=[IO.Path]::GetFullPath((Join-Path $runtime (Get-BridgeRelativePath $config $oldRuntime)))}
   $marker.config=$config
  }
  $marker|Add-Member -NotePropertyName install_root -NotePropertyValue $(if((Split-Path -Leaf $runtime)-eq 'app'){Split-Path -Parent $runtime}else{$runtime}) -Force
 }else{
  if($marker.paths_version-notin @(0,1)){throw 'The portable marker path version is unsupported.'}
  $stamp=Get-BridgeDeploymentId $runtime
  if(-not $stamp-or $stamp-ne [string]$marker.deployment_id){throw 'The role marker belongs to another deployment.'}
  if($marker.config-eq '@external'){
   $binding=Get-BridgeDeploymentBinding $runtime
   $external=if($binding){[string]$binding.($Role+'ExternalConfig')}else{''}
   if($external){$marker.config=Resolve-BridgeConfigPath $external $path $runtime}
   elseif([string]$marker.machine_id-ne (Get-BridgeInstallationMachineIdentity)){$marker.config=$null;$marker|Add-Member -NotePropertyName unresolved_external -NotePropertyValue $true -Force}
   else{throw 'The external configuration registration is missing on this computer.'}
  }elseif($marker.config){$marker.config=Resolve-BridgeConfigPath ([string]$marker.config) $path $runtime}
  $marker.install_root=Resolve-BridgeConfigPath ([string]$marker.install_root) $path $runtime
 }
 $marker
}
function Register-BridgeDeploymentBinding {
 param([string]$Runtime,[string]$InstallRoot,[hashtable]$RoleConfigs=@{})
 $runtime=[IO.Path]::GetFullPath($Runtime)
 $root=[IO.Path]::GetFullPath($InstallRoot)
 $identifier=Get-BridgeDeploymentId $runtime -Create
 $machine=Get-BridgeInstallationMachineIdentity
 $manifest=Join-Path $runtime 'SHA256.json'
 if(-not(Test-Path -LiteralPath $manifest -PathType Leaf)){throw 'The complete release payload manifest is required before registering this deployment.'}
 $registration='HKLM:\Software\USBtoNetworkBridge\Deployments\'+$identifier
 $existing=Get-ItemProperty -LiteralPath $registration -ErrorAction SilentlyContinue
 if($existing-and $existing.MachineId-and $existing.MachineId-ne $machine){throw 'The protected deployment registration belongs to another Windows installation.'}
 $values=[ordered]@{DeploymentId=$identifier;MachineId=$machine;RuntimeDirectory=$runtime;InstallRoot=$root;PayloadManifestSHA256=(Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash.ToLowerInvariant()}
 foreach($role in @('Server','Client')){
  foreach($suffix in @('Config','ExternalConfig')){if($existing-and $existing.PSObject.Properties[$role+$suffix]){$values[$role+$suffix]=[string]$existing.($role+$suffix)}}
 }
 foreach($role in $RoleConfigs.Keys){
  if($role-notin @('Server','Client')){throw 'Unknown deployment role.'}
  $config=Resolve-BridgeConfigPath ([string]$RoleConfigs[$role]) (Join-Path $runtime 'deployment.local.json') $runtime
  $locator=Get-BridgeRoleConfigLocator $config $runtime
  $values[$role+'Config']=$locator
  if($locator-eq '@external'){$values[$role+'ExternalConfig']=$config}else{$values.Remove($role+'ExternalConfig')}
 }
 New-Item -Path $registration -Force|Out-Null
 foreach($name in $values.Keys){New-ItemProperty -LiteralPath $registration -Name $name -Value ([string]$values[$name]) -PropertyType String -Force|Out-Null}
 foreach($role in @('Server','Client')){if($values[$role+'Config']-and $values[$role+'Config']-ne '@external'){Remove-ItemProperty -LiteralPath $registration -Name ($role+'ExternalConfig') -ErrorAction SilentlyContinue}}
 $acl=New-Object Security.AccessControl.RegistrySecurity
 $acl.SetAccessRuleProtection($true,$false)
 foreach($id in @('S-1-5-18','S-1-5-32-544')){$sid=New-Object Security.Principal.SecurityIdentifier($id);$acl.AddAccessRule((New-Object Security.AccessControl.RegistryAccessRule($sid,'FullControl','ContainerInherit','None','Allow')))}
 $readers=New-Object Security.Principal.SecurityIdentifier('S-1-5-32-545')
 $acl.AddAccessRule((New-Object Security.AccessControl.RegistryAccessRule($readers,'ReadKey','ContainerInherit','None','Allow')))
 Set-Acl -LiteralPath $registration -AclObject $acl
 Get-ItemProperty -LiteralPath $registration
}
function Assert-BridgeInstallationTree([string]$Path) {
 $current=Get-Item -LiteralPath $Path -Force -ErrorAction Stop
 while($current){
  if($current.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Installation code must not pass through a symbolic link or junction.'}
  $current=if($current-is [IO.DirectoryInfo]){$current.Parent}else{$current.Directory}
 }
}
function Assert-BridgeRelocatedRuntime {
 param([string]$InstallRoot,[string]$Runtime,$Binding)
 $root=[IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
 $runtime=[IO.Path]::GetFullPath($Runtime).TrimEnd('\')
 if(-not [string]::Equals($runtime,(Join-Path $root 'app'),[StringComparison]::OrdinalIgnoreCase)){throw 'The moved runtime must be the installation app directory.'}
 Assert-BridgeInstallationTree $root
 Assert-BridgeInstallationTree $runtime
 if(-not $Binding-or $Binding.MachineId-ne (Get-BridgeInstallationMachineIdentity)){throw 'The deployment registration does not belong to this Windows installation.'}
 $expected=[string]$Binding.PayloadManifestSHA256
 if($expected-notmatch '^[a-fA-F0-9]{64}$'){throw 'The protected payload manifest hash is invalid.'}
 $manifest=Join-Path $runtime 'SHA256.json'
 Assert-BridgeInstallationTree $manifest
 if((Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash-ne $expected){throw 'The moved payload manifest differs from the protected registration.'}
 $records=Read-BridgeInstallationJson $manifest
 if($records-isnot [array]-or -not $records.Count-or $records.Count-gt 4096){throw 'The payload manifest has an invalid file count.'}
 $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 foreach($record in $records){
  $name=[string]$record.File
  if(-not $name-or [IO.Path]::IsPathRooted($name)-or $name-match '^[A-Za-z]:'-or $name.Split([char[]]@('/','\'))-contains '..'-or $name.Contains(':')){throw 'The payload manifest has an unsafe relative filename.'}
  $relative=$name.Replace('/','\')
  if($relative-notin @('USB-to-Network-Bridge-Client.exe','USB-to-Network-Bridge-Server.exe')-and -not $relative.StartsWith('app\',[StringComparison]::OrdinalIgnoreCase)){throw 'The payload manifest has an unexpected root file.'}
  if(-not $seen.Add($relative)){throw 'The payload manifest contains a duplicate file.'}
  $file=[IO.Path]::GetFullPath((Join-Path $root $relative))
  if(-not(Test-BridgePathWithin $file $root)){throw 'The payload manifest escaped the installation directory.'}
  if(-not(Test-Path -LiteralPath $file -PathType Leaf)){throw ('A release payload file is missing: '+$relative)}
  Assert-BridgeInstallationTree $file
  if([string]$record.SHA256-notmatch '^[a-fA-F0-9]{64}$'-or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash-ne [string]$record.SHA256){throw ('A release payload file failed verification: '+$relative)}
  if($record.PSObject.Properties['Bytes']-and (Get-Item -LiteralPath $file).Length-ne [long]$record.Bytes){throw ('A release payload file has an unexpected length: '+$relative)}
 }
 $required=@('USB-to-Network-Bridge-Client.exe','USB-to-Network-Bridge-Server.exe','app\usbbridge.exe','app\usbbridge-client.exe','app\usbbridge-server.exe','app\usbbridge-agent.exe','app\usbbridge-camera.exe','app\Open-Installation.ps1','app\Configure-Installation.ps1','app\FirstRun-Identity.ps1','app\Portable-Paths.ps1','app\Portable-Installation.ps1','app\Relocate-Installation.ps1','app\Maintain-Installation.ps1','app\New-BackendMutex.ps1','app\Update-PortableSettings.ps1','app\Set-Autostart.ps1','app\Switch-Backend.ps1','app\Configure-Server.ps1','app\Publish-Device.ps1','app\Set-ServerState.ps1','app\Set-VideoMode.ps1','app\Restart-VideoEndpoint.ps1','app\Ensure-Exports.ps1','app\Verify-DriverInstaller.ps1','app\Ensure-ProductionDrivers.ps1','app\Install-ProductionDrivers.ps1','app\Download-Drivers.ps1','app\driver-installers.json','app\tools\driver-installers\USBip-0.9.8.1-x64.exe','app\tools\driver-installers\usbipd-win_5.3.0_x64.msi')
 foreach($name in $required){if(-not $seen.Contains($name)){throw ('The verified manifest omits a required application file: '+$name)}}
}
