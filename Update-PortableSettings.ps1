# Normalize only this verified installation's settings; never delete source secrets.
param([string]$InstallRoot,[ValidateSet('Server','Client')][string]$Role,[string]$Config,[switch]$Elevate,[switch]$FunctionsOnly)
$ErrorActionPreference='Stop'
function Read-BridgeNormalizationJson([string]$Path) {
 if(-not(Test-Path -LiteralPath $Path -PathType Leaf)){return $null}
 $item=Get-Item -LiteralPath $Path
 if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'A settings file must not be a junction or symlink.'}
 if([UInt64]$item.Length-gt [UInt64]1048576){throw 'An installation settings file is too large.'}
 $content=[IO.File]::ReadAllText($item.FullName)
 [pscustomobject]@{path=$item.FullName;content=$content;value=($content|ConvertFrom-Json)}
}
function Write-BridgeNormalizationText([string]$Path,[string]$Content) {
 $temporary=Join-Path ([IO.Path]::GetDirectoryName($Path)) ('.portable-settings-'+[guid]::NewGuid().ToString('N')+'.tmp')
 try {
  [IO.File]::WriteAllText($temporary,$Content,[Text.UTF8Encoding]::new($false))
  if(Test-Path -LiteralPath $Path){[IO.File]::Replace($temporary,$Path,[Management.Automation.Language.NullString]::Value)}else{[IO.File]::Move($temporary,$Path)}
 }finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Convert-BridgeNormalizationLegacyPath([string]$Value,[string]$OldRuntime,[string]$Runtime) {
 if(-not $Value-or -not $OldRuntime-or -not [IO.Path]::IsPathRooted($Value)){return $Value}
 $old=[IO.Path]::GetFullPath($OldRuntime).TrimEnd('\','/')
 $path=[IO.Path]::GetFullPath($Value)
 if([string]::Equals($path.TrimEnd('\','/'),$old,[StringComparison]::OrdinalIgnoreCase)){return [IO.Path]::GetFullPath($Runtime)}
 if($path.StartsWith($old+'\',[StringComparison]::OrdinalIgnoreCase)){return ([IO.Path]::GetFullPath($Runtime).TrimEnd('\','/')+$path.Substring($old.Length))}
 $Value
}
function Update-BridgeNormalizationLegacyPaths($Node,[string]$OldRuntime,[string]$Runtime) {
 if($null-eq $Node-or $Node-is [string]){return}
 if($Node-is [Collections.IEnumerable]-and $Node-isnot [pscustomobject]){foreach($child in $Node){Update-BridgeNormalizationLegacyPaths $child $OldRuntime $Runtime};return}
 foreach($property in $Node.PSObject.Properties){
  if($property.Name-in @('runtime','profile','key_file','credential_file')-and $property.Value-is [string]){$property.Value=Convert-BridgeNormalizationLegacyPath $property.Value $OldRuntime $Runtime}
  else{Update-BridgeNormalizationLegacyPaths $property.Value $OldRuntime $Runtime}
 }
}
function Set-BridgeNormalizationPrivateAcl([string]$Path,[switch]$Directory) {
 # Modify only the DACL. Replacing a freshly constructed full security
 # descriptor also asks for SACL privileges on Windows PowerShell 5.
 $acl=Get-Acl -LiteralPath $Path
 $acl.SetAccessRuleProtection($true,$false)
 $identity=[Security.Principal.WindowsIdentity]::GetCurrent().User
 foreach($existing in @($acl.Access)){$acl.RemoveAccessRuleAll($existing)}
 $sids=@($identity,(New-Object Security.Principal.SecurityIdentifier('S-1-5-18')),(New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
 foreach($sid in $sids){
  $rule=if($Directory){New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')}else{New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','Allow')}
  $acl.AddAccessRule($rule)
 }
 if($Directory){[IO.Directory]::SetAccessControl($Path,$acl)}else{[IO.File]::SetAccessControl($Path,$acl)}
}
function Copy-BridgeNormalizationSecret([string]$Source,[string]$Config,[Collections.IDictionary]$Copies,$Created) {
 $source=[IO.Path]::GetFullPath($Source)
 if(-not(Test-Path -LiteralPath $source -PathType Leaf)){throw 'A configured external credential is missing. Restore it before preparing portable settings.'}
 $item=Get-Item -LiteralPath $source
 if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'An external credential must not be a junction or symlink.'}
 if([UInt64]$item.Length-gt [UInt64]1048576){throw 'A configured credential is too large to copy into portable settings.'}
 $parent=Get-BridgeConfigDirectory $Config
 $directory=Join-Path $parent 'portable-credentials'
 if((Test-Path -LiteralPath $directory)-and ((Get-Item -LiteralPath $directory).Attributes-band [IO.FileAttributes]::ReparsePoint)){throw 'The portable credential directory must not be a junction or symlink.'}
 $cacheKey=$source.ToLowerInvariant()+'|'+$directory.ToLowerInvariant()
 if($Copies.Contains($cacheKey)){return [string]$Copies[$cacheKey]}
 New-Item -ItemType Directory -Path $directory -Force|Out-Null
 Set-BridgeNormalizationPrivateAcl $directory -Directory
 $extension=if([IO.Path]::GetExtension($source)-eq '.key'){'.key'}else{'.credential'}
 $target=Join-Path $directory ([guid]::NewGuid().ToString('N')+$extension)
 $secretInputStream=$null;$secretOutputStream=$null
 try {
  # A new private destination receives its ACL before any credential bytes.
  $secretOutputStream=[IO.File]::Open($target,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  $secretOutputStream.Dispose();$secretOutputStream=$null
  $Created.Add($target)
  Set-BridgeNormalizationPrivateAcl $target
  $secretInputStream=[IO.File]::Open($source,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
  if([UInt64]$secretInputStream.Length-gt [UInt64]1048576){throw 'A configured credential changed size while preparing portable settings.'}
  $secretOutputStream=[IO.File]::Open($target,[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::None)
  $secretInputStream.CopyTo($secretOutputStream,65536)
  $secretOutputStream.Flush($true)
 }finally{if($null-ne $secretInputStream){$secretInputStream.Dispose()};if($null-ne $secretOutputStream){$secretOutputStream.Dispose()}}
 $Copies[$cacheKey]=$target
 $target
}
function Update-BridgeNormalizationSecrets($Node,[string]$Config,[string]$Runtime,[Collections.IDictionary]$Copies,$Created) {
 if($null-eq $Node-or $Node-is [string]){return}
 if($Node-is [Collections.IEnumerable]-and $Node-isnot [pscustomobject]){foreach($child in $Node){Update-BridgeNormalizationSecrets $child $Config $Runtime $Copies $Created};return}
 $directory=Get-BridgeConfigDirectory $Config
 foreach($property in $Node.PSObject.Properties){
  if($property.Name-in @('key_file','credential_file')-and $property.Value-is [string]-and $property.Value){
   $path=[string]$property.Value
   if(-not(Test-BridgePathWithin $path $Runtime)-and -not(Test-BridgePathWithin $path $directory)){$property.Value=Copy-BridgeNormalizationSecret $path $Config $Copies $Created}
  }else{Update-BridgeNormalizationSecrets $property.Value $Config $Runtime $Copies $Created}
 }
}
function New-BridgeNormalizationWrite($Document,$Value) {
 $before=$Document.value|ConvertTo-Json -Depth 24 -Compress
 $after=$Value|ConvertTo-Json -Depth 24 -Compress
 if($before-ceq $after){return $null}
 $content=$Value|ConvertTo-Json -Depth 24
 if([UInt64][Text.Encoding]::UTF8.GetByteCount($content)-gt [UInt64]1048576){throw 'Normalized installation settings exceed the size limit.'}
 [pscustomobject]@{path=$Document.path;content=$content;before=$Document.content}
}
function Convert-BridgeRoleSettings {
 param([Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,[Parameter(Mandatory=$true)][string]$Runtime,[Parameter(Mandatory=$true)][string]$Config,[string]$OldRuntime,[string]$InstallRoot)
 $runtime=[IO.Path]::GetFullPath($Runtime);$config=[IO.Path]::GetFullPath($Config)
 . (Join-Path $runtime 'Portable-Paths.ps1')
 if(-not $OldRuntime){$OldRuntime=$runtime}
 $main=Read-BridgeNormalizationJson $config
 if(-not $main){throw 'The verified role configuration is missing.'}
 $documents=New-Object 'Collections.Generic.List[object]'
 $documents.Add($main)
 $prefix=[IO.Path]::GetFileNameWithoutExtension($config)-replace '\.local$',''
 foreach($videoRole in @('receive','send','local-send')){
  $path=Join-Path (Get-BridgeConfigDirectory $config) ($prefix+'.video-'+$videoRole+'.local.json')
  $document=Read-BridgeNormalizationJson $path
  if($document){$documents.Add($document)}
 }
 $writes=New-Object 'Collections.Generic.List[object]';$created=New-Object 'Collections.Generic.List[string]';$copies=@{};$applied=New-Object 'Collections.Generic.List[object]'
 try {
  foreach($document in $documents){
   # Work on a clone so equality comparisons and rollback retain the input.
   $value=$document.content|ConvertFrom-Json
   Update-BridgeNormalizationLegacyPaths $value $OldRuntime $runtime
   $value=Resolve-BridgeConfigPaths $value $document.path $runtime
   Update-BridgeNormalizationSecrets $value $document.path $runtime $copies $created
   if($document.path-eq $config-and $Role-eq 'Server'){
    if($value.runtime-and -not [string]::Equals(([IO.Path]::GetFullPath($value.runtime)).TrimEnd('\'),$runtime.TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)){throw 'The server configuration belongs to another application directory.'}
    $value|Add-Member -NotePropertyName runtime -NotePropertyValue $runtime -Force
   }
   $value=ConvertTo-BridgePortableConfig $value $document.path $runtime
   $write=New-BridgeNormalizationWrite $document $value
   if($write){$writes.Add($write)}
  }
  $metadata=Read-BridgeNormalizationJson (Join-Path $runtime ('autostart-'+$Role.ToLowerInvariant()+'.local.json'))
  if($metadata-and $metadata.value.Config){
   $selected=Convert-BridgeNormalizationLegacyPath ([string]$metadata.value.Config) $OldRuntime $runtime
   $selected=Resolve-BridgeConfigPath $selected $metadata.path $runtime
   if([string]::Equals($selected,$config,[StringComparison]::OrdinalIgnoreCase)){
    $value=$metadata.content|ConvertFrom-Json
    Update-BridgeNormalizationLegacyPaths $value $OldRuntime $runtime
    $value=ConvertTo-BridgePortableConfig $value $metadata.path $runtime
    $value.Config=ConvertTo-BridgePortablePath $config $metadata.path $runtime
    $write=New-BridgeNormalizationWrite $metadata $value
    if($write){$writes.Add($write)}
   }
  }
  foreach($write in $writes){Write-BridgeNormalizationText $write.path $write.content;$applied.Add($write)}
 }catch{
  $failure=$_
  $restored=$true
  foreach($write in $applied){try{Write-BridgeNormalizationText $write.path $write.before}catch{$restored=$false}}
  # If restoring a config failed, retain the private copy it may still use.
  if($restored){foreach($path in $created){if(Test-Path -LiteralPath $path){Remove-Item -LiteralPath $path -Force}}}
  throw $failure
 }
 [pscustomobject]@{role=$Role;config=$config;files_changed=$writes.Count;credentials_copied=$created.Count}
}
function Update-BridgeOwnedInstalledMarker([string]$Role,[string]$Runtime,[string]$Config,[string]$OldRuntime,[string]$MachineIdentity) {
 $path=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('USB to Network Bridge\'+$Role+'\installation.local.json')
 $document=Read-BridgeNormalizationJson $path
 if(-not $document-or -not $document.value.initialized){return}
 $value=$document.value
 if($value.machine_id-and $value.machine_id-ne $MachineIdentity){return}
 if(-not $value.runtime){return}
 $selected=Convert-BridgeNormalizationLegacyPath ([string]$value.runtime) $OldRuntime $Runtime
 $selected=Resolve-BridgeConfigPath $selected $path $Runtime
 if(-not [string]::Equals($selected.TrimEnd('\'),$Runtime.TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)){return}
 if($value.config){
  $selected=Convert-BridgeNormalizationLegacyPath ([string]$value.config) $OldRuntime $Runtime
  $selected=Resolve-BridgeConfigPath $selected $path $Runtime
  if(-not [string]::Equals($selected,$Config,[StringComparison]::OrdinalIgnoreCase)){return}
 }
 $value.runtime='@app'
 if($value.PSObject.Properties['config']){$value.config=ConvertTo-BridgePortablePath $Config $path $Runtime}
 if($value.PSObject.Properties['install_root']){$value.PSObject.Properties.Remove('install_root')}
 $value|Add-Member -NotePropertyName paths_version -NotePropertyValue 1 -Force
 Write-BridgeNormalizationText $path ($value|ConvertTo-Json -Depth 24)
}
if($FunctionsOnly){return}
if(-not $InstallRoot){throw 'Specify the deployed application folder with -InstallRoot.'}
$InstallRoot=[IO.Path]::GetFullPath($InstallRoot).TrimEnd('\','/')
$runtime=if(Test-Path -LiteralPath (Join-Path $InstallRoot 'app\FirstRun-Identity.ps1')){Join-Path $InstallRoot 'app'}else{$InstallRoot}
. (Join-Path $runtime 'FirstRun-Identity.ps1')
. (Join-Path $runtime 'Portable-Paths.ps1')
if($Config){$Config=[IO.Path]::GetFullPath($Config);if(-not $Role){throw 'An explicit configuration requires -Role.'}}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'Administrator rights are required to prepare this installation for relocation.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-InstallRoot',('"'+$InstallRoot+'"'))
 if($Role){$arguments+=@('-Role',$Role)}
 if($Config){$arguments+=@('-Config',('"'+$Config+'"'))}
 $child=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $child.WaitForExit()
 if($child.ExitCode-ne 0){throw 'Portable settings preparation failed.'}
 return
}
$machine=Get-BridgeMachineIdentity
$roleConfigs=@{}
$roles=@('Server','Client')
$plans=New-Object 'Collections.Generic.List[object]'
foreach($selectedRole in $roles){
 $raw=Read-BridgeNormalizationJson (Get-BridgeMachineMarkerPath $selectedRole $runtime)
 $marker=Get-BridgeMachineMarker $selectedRole $runtime
 if($marker-and $marker.machine_id-and $marker.machine_id-ne $machine){continue}
 $selectedConfig=if($Config-and $selectedRole-eq $Role){$Config}elseif($marker-and $marker.config){[string]$marker.config}else{$null}
 if(-not $selectedConfig){continue}
 $verified=$marker-and $marker.machine_id-eq $machine-and $marker.initialized-and $marker.role-eq $selectedRole
 if(-not $verified){$verified=(Test-BridgeOwnedRoleTask $selectedRole $runtime $selectedConfig)-or (Test-BridgeOwnedRoleProcess $selectedRole $runtime $selectedConfig)}
 if(-not $verified){continue}
 $oldRuntime=if($raw-and $raw.value.runtime-and [IO.Path]::IsPathRooted([string]$raw.value.runtime)){[string]$raw.value.runtime}else{$runtime}
 $plans.Add([pscustomobject]@{role=$selectedRole;config=$selectedConfig;old_runtime=$oldRuntime})
}
if($Role-and -not @($plans|Where-Object role -eq $Role).Count){throw 'The requested role could not be verified for this computer; its settings were not adopted.'}
foreach($plan in $plans){
 $null=Convert-BridgeRoleSettings -Role $plan.role -Runtime $runtime -Config $plan.config -OldRuntime $plan.old_runtime -InstallRoot $InstallRoot
 Update-BridgeOwnedInstalledMarker $plan.role $runtime $plan.config $plan.old_runtime $machine
 $null=Write-BridgeMachineMarker -Role $plan.role -Runtime $runtime -Config $plan.config -InstallRoot $InstallRoot -AdoptExisting
 $roleConfigs[$plan.role]=$plan.config
}
if($roleConfigs.Count){$null=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $InstallRoot -RoleConfigs $roleConfigs}
return
