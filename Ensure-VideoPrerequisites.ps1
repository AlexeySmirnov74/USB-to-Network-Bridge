# Local-only Unity Capture setup. Dot-sourcing with -FunctionsOnly has no side effects.
param(
 [ValidateSet('Server','Client')][string]$Role='Client',
 [string]$RuntimeDirectory=$PSScriptRoot,
 [string]$FilterDll,
 [switch]$Elevate,
 [switch]$FunctionsOnly
)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
function Get-BridgeUnitySlots {
 @(
  [pscustomobject]@{Guid='{5C2CD55C-92AD-4999-8666-912BD3E70010}';Name='Unity Capture Server'},
  [pscustomobject]@{Guid='{5C2CD55C-92AD-4999-8666-912BD3E70012}';Name='Unity Capture Client'}
 )
}
function Test-BridgeUnitySamePath([string]$First,[string]$Second) {
 if(-not $First-or -not $Second){return $false}
 [string]::Equals([IO.Path]::GetFullPath($First).TrimEnd('\'),[IO.Path]::GetFullPath($Second).TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)
}
function Assert-BridgeUnityPlainPath([string]$Path) {
 $node=Get-Item -LiteralPath ([IO.Path]::GetFullPath($Path)) -Force
 while($node){
  if($node.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Unity Capture paths must not contain a junction or symbolic link.'}
  $node=if($node.PSIsContainer){$node.Parent}else{$node.Directory}
 }
}
function Test-BridgeUnityX64Dll([string]$Path) {
 if(-not $Path-or [IO.Path]::GetFileName($Path)-ine 'UnityCaptureFilter64.dll'-or -not(Test-Path -LiteralPath $Path -PathType Leaf)){return $false}
 $reader=$null
 try {
  Assert-BridgeUnityPlainPath $Path
  $reader=[IO.BinaryReader]::new([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
  if($reader.BaseStream.Length-lt 256-or $reader.BaseStream.Length-gt 16777216-or $reader.ReadUInt16()-ne 0x5a4d){return $false}
  $reader.BaseStream.Position=0x3c;$offset=$reader.ReadUInt32()
  if($offset-gt ($reader.BaseStream.Length-26)){return $false}
  $reader.BaseStream.Position=$offset
  if($reader.ReadUInt32()-ne 0x4550-or $reader.ReadUInt16()-ne 0x8664){return $false}
  $reader.BaseStream.Position=$offset+22
  if(($reader.ReadUInt16()-band 0x2000)-eq 0){return $false}
  $reader.ReadUInt16()-eq 0x20b
 }catch{return $false}finally{if($reader){$reader.Dispose()}}
}
function Get-BridgeUnityManifest([string]$RuntimeDirectory) {
 $path=Join-Path $RuntimeDirectory 'unity-capture.json'
 if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw 'The complete offline app folder is required: unity-capture.json is missing. No download was attempted.'}
 Assert-BridgeUnityPlainPath $path
 if((Get-Item -LiteralPath $path).Length-gt 65536){throw 'Unity Capture manifest is too large.'}
 $manifest=Get-Content -LiteralPath $path -Raw -Encoding UTF8|ConvertFrom-Json
 if($manifest.schema-ne 1-or $manifest.file-cne 'components/unity-capture/UnityCaptureFilter64.dll'-or $manifest.architecture-cne 'x64'-or [string]$manifest.sha256-notmatch '^[0-9a-fA-F]{64}$'){throw 'The bundled Unity Capture manifest is invalid.'}
 $manifest
}
function Assert-BridgeUnityPinnedDll([string]$Path,$Manifest) {
 if(-not(Test-BridgeUnityX64Dll $Path)){throw 'The bundled UnityCaptureFilter64.dll is missing or is not an x64 DLL. No download was attempted.'}
 $actual=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
 if($actual-ine [string]$Manifest.sha256){throw 'Unity Capture SHA256 mismatch. Restore the complete offline package; no registration or download was attempted.'}
 [IO.Path]::GetFullPath($Path)
}
function Get-BridgeUnityRegistryState {
 # Inspect both 64-bit registration hives; an HKCU override can shadow HKLM.
 $slots=@(Get-BridgeUnitySlots);$result=@()
 $propertyGuid='{5C2CD55C-92AD-4999-8666-912BD3E70011}'
 foreach($hive in @([Microsoft.Win32.RegistryHive]::LocalMachine,[Microsoft.Win32.RegistryHive]::CurrentUser)){
  $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,[Microsoft.Win32.RegistryView]::Registry64)
  try {
   foreach($slot in $slots){
    $key=$base.OpenSubKey(('SOFTWARE\Classes\CLSID\'+$slot.Guid),$false)
    if(-not $key){continue}
    try {
     $server=$key.OpenSubKey('InprocServer32',$false)
     try{$dll=if($server){[string]$server.GetValue('','')}else{''}}finally{if($server){$server.Dispose()}}
     $result+=[pscustomobject]@{Hive=$hive.ToString();Kind='Class';Guid=$slot.Guid;Name=[string]$key.GetValue('','');Path=$dll}
    }finally{$key.Dispose()}
   }
   # DllRegisterServer also writes the shared filter property-page class.
   # It has no video-category instance, but must be included before registration
   # so a foreign installation cannot be silently overwritten through this CLSID.
   $key=$base.OpenSubKey(('SOFTWARE\Classes\CLSID\'+$propertyGuid),$false)
   if($key){
    try {
     $server=$key.OpenSubKey('InprocServer32',$false)
     try{$dll=if($server){[string]$server.GetValue('','')}else{''}}finally{if($server){$server.Dispose()}}
     $result+=[pscustomobject]@{Hive=$hive.ToString();Kind='PropertiesClass';Guid=$propertyGuid;Name=[string]$key.GetValue('','');Path=$dll}
    }finally{$key.Dispose()}
   }
   $category=$base.OpenSubKey('SOFTWARE\Classes\CLSID\{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance',$false)
   if($category){
    try {
     $names=$category.GetSubKeyNames();if($names.Count-gt 512){throw 'Too many video capture registrations; Unity setup was not changed.'}
     foreach($name in $names){
      $key=$category.OpenSubKey($name,$false)
      if(-not $key){continue}
      try {
       $friendly=[string]$key.GetValue('FriendlyName','');$clsid=[string]$key.GetValue('CLSID','')
       if($name-in $slots.Guid-or $friendly-in $slots.Name-or $clsid-in $slots.Guid){$result+=[pscustomobject]@{Hive=$hive.ToString();Kind='Instance';Guid=$name;Name=$friendly;Clsid=$clsid;Path=''}}
      }finally{$key.Dispose()}
     }
    }finally{$category.Dispose()}
   }
  }finally{$base.Dispose()}
 }
 $result
}
function Get-BridgeUnityState([string]$RuntimeDirectory=$PSScriptRoot) {
 $entries=@(Get-BridgeUnityRegistryState);$slots=@(Get-BridgeUnitySlots)
 if(@($entries|Where-Object {$_.Hive-ne 'LocalMachine'}).Count){return [pscustomobject]@{Ready=$false;Disposition='Conflict';Filter='';Reason='A per-user Unity Capture registration shadows the machine slots. It was not changed.';Entries=$entries}}
 foreach($slot in $slots){
  $sameName=@($entries|Where-Object {$_.Kind-eq 'Instance'-and $_.Name-eq $slot.Name})
  if($sameName.Count-gt 1-or @($sameName|Where-Object {$_.Guid-ne $slot.Guid-or $_.Clsid-ne $slot.Guid}).Count){return [pscustomobject]@{Ready=$false;Disposition='Conflict';Filter='';Reason='Unity Capture source names are ambiguous or belong to another filter. They were not changed.';Entries=$entries}}
 }
 $paths=@();$complete=$true
 foreach($slot in $slots){
  $class=@($entries|Where-Object {$_.Kind-eq 'Class'-and $_.Guid-eq $slot.Guid})
  $instance=@($entries|Where-Object {$_.Kind-eq 'Instance'-and $_.Guid-eq $slot.Guid})
  if($class.Count-ne 1-or $instance.Count-ne 1){$complete=$false;continue}
  if($class[0].Name-cne $slot.Name-or $instance[0].Name-cne $slot.Name-or $instance[0].Clsid-ne $slot.Guid){$complete=$false;continue}
  $paths+=[string]$class[0].Path
 }
 if($complete-and $paths.Count-eq 2-and (Test-BridgeUnitySamePath $paths[0] $paths[1])-and (Test-BridgeUnityX64Dll $paths[0])){
  $properties=@($entries|Where-Object {$_.Kind-eq 'PropertiesClass'})
  if($properties.Count-gt 1-or ($properties.Count-eq 1-and -not(Test-BridgeUnitySamePath $properties[0].Path $paths[0]))){return [pscustomobject]@{Ready=$false;Disposition='Conflict';Filter='';Reason='The Unity Capture property-page class belongs to another installation. It was not changed.';Entries=$entries}}
  # A missing optional property UI does not invalidate two working video slots.
  return [pscustomobject]@{Ready=$true;Disposition='Ready';Filter=$paths[0];Reason='';Entries=$entries}
 }
 [pscustomobject]@{Ready=$false;Disposition=$(if($entries.Count){'Partial'}else{'Missing'});Filter='';Reason='Unity Capture Server and Unity Capture Client are not both registered.';Entries=$entries}
}
function Test-BridgeUnityCaptureReadiness([string]$RuntimeDirectory=$PSScriptRoot) {
 try{Get-BridgeUnityState $RuntimeDirectory}catch{[pscustomobject]@{Ready=$false;Disposition='Conflict';Filter='';Reason=$_.Exception.Message;Entries=@()}}
}
function Get-BridgeUnityStableRoot {
 Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge\Components\UnityCapture'
}
function Protect-BridgeUnityDirectory([string]$Path) {
 if(-not(Test-Path -LiteralPath $Path)){New-Item -ItemType Directory -Path $Path -Force|Out-Null}
 Assert-BridgeUnityPlainPath $Path
 $acl=New-Object Security.AccessControl.DirectorySecurity
 $acl.SetAccessRuleProtection($true,$false)
 $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
 foreach($entry in @(@('S-1-5-18','FullControl'),@('S-1-5-32-544','FullControl'),@('S-1-5-32-545','ReadAndExecute'))){
  $sid=New-Object Security.Principal.SecurityIdentifier($entry[0])
  $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,$entry[1],'ContainerInherit,ObjectInherit','None','Allow')))
 }
 Set-Acl -LiteralPath $Path -AclObject $acl
}
function Protect-BridgeUnityFile([string]$Path) {
 Assert-BridgeUnityPlainPath $Path
 $acl=New-Object Security.AccessControl.FileSecurity
 $acl.SetAccessRuleProtection($true,$false)
 $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
 foreach($entry in @(@('S-1-5-18','FullControl'),@('S-1-5-32-544','FullControl'),@('S-1-5-32-545','ReadAndExecute'))){
  $sid=New-Object Security.Principal.SecurityIdentifier($entry[0])
  $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,$entry[1],'Allow')))
 }
 Set-Acl -LiteralPath $Path -AclObject $acl
}
function Assert-BridgeUnitySharedDirectory([string]$Path) {
 Assert-BridgeUnityPlainPath $Path
 $acl=Get-Acl -LiteralPath $Path
 $trusted=@('S-1-5-18','S-1-5-32-544')
 if($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value-notin $trusted){throw 'The shared Bridge component parent has an untrusted owner. Its permissions were preserved; install Bridge in its protected standard location before preparing video.'}
 $mask=[Security.AccessControl.FileSystemRights]::Delete-bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles-bor [Security.AccessControl.FileSystemRights]::ChangePermissions-bor [Security.AccessControl.FileSystemRights]::TakeOwnership
 foreach($rule in $acl.Access){
  if($rule.AccessControlType-ne 'Allow'-or ($rule.PropagationFlags-band [Security.AccessControl.PropagationFlags]::InheritOnly)){continue}
  $sid=$rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
  if($sid-notin $trusted-and ($rule.FileSystemRights-band $mask)-ne 0){throw 'The shared Bridge component parent permits unprivileged path replacement. Its existing permissions were preserved; repair that folder security before preparing video.'}
 }
}
function Initialize-BridgeUnityDirectories([string]$Root,[string]$Digest) {
 $common=Split-Path -Parent (Split-Path -Parent $Root)
 # This parent also contains role settings and may have deliberate service-
 # account grants. Do not reset an existing shared DACL or touch its children.
 if(Test-Path -LiteralPath $common){Assert-BridgeUnitySharedDirectory $common}else{Protect-BridgeUnityDirectory $common}
 foreach($directory in @((Split-Path -Parent $Root),$Root,(Join-Path $Root $Digest.ToLowerInvariant()))){Protect-BridgeUnityDirectory $directory}
}
function Install-BridgeUnityStableDll([string]$Source,$Manifest) {
 $root=Get-BridgeUnityStableRoot
 Initialize-BridgeUnityDirectories $root ([string]$Manifest.sha256)
 $target=Join-Path (Join-Path $root ([string]$Manifest.sha256).ToLowerInvariant()) 'UnityCaptureFilter64.dll'
 if(Test-Path -LiteralPath $target){Protect-BridgeUnityFile $target;$null=Assert-BridgeUnityPinnedDll $target $Manifest;return $target}
 # Keep the verified source open without write/delete sharing while copying.
 $inputFile=[IO.File]::Open($Source,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
 $temporary=Join-Path ([IO.Path]::GetDirectoryName($target)) ('.capture-'+[guid]::NewGuid().ToString('N')+'.tmp')
 try{
  $outputFile=[IO.File]::Open($temporary,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  try{$inputFile.CopyTo($outputFile)}finally{$outputFile.Dispose()}
  if((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash-ine [string]$Manifest.sha256){throw 'Unity Capture changed during its local copy; registration was not attempted.'}
  [IO.File]::Move($temporary,$target)
 }finally{$inputFile.Dispose();if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
 Protect-BridgeUnityFile $target
 $null=Assert-BridgeUnityPinnedDll $target $Manifest
 $target
}
function Register-BridgeUnitySlots([string]$FilterDll,$Manifest) {
 $null=Assert-BridgeUnityPinnedDll $FilterDll $Manifest
 $lock=[IO.File]::Open($FilterDll,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
 try {
  $registrar=Join-Path $env:SystemRoot 'System32\regsvr32.exe'
  $operation=Start-Process -FilePath $registrar -ArgumentList @('/s','/i:UnityCaptureDevices=2',('"'+$FilterDll+'"')) -WindowStyle Hidden -Wait -PassThru
  if($operation.ExitCode-ne 0){throw ('Bundled Unity Capture registration failed: '+$operation.ExitCode)}
  $classes='HKLM:\SOFTWARE\Classes\CLSID';$category=Join-Path $classes '{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance'
  foreach($slot in @(Get-BridgeUnitySlots)){
   $class=Join-Path $classes $slot.Guid;$instance=Join-Path $category $slot.Guid
   if(-not(Test-Path -LiteralPath $instance)-or -not(Test-Path -LiteralPath ($class+'\InprocServer32'))-or -not(Test-BridgeUnitySamePath ((Get-Item -LiteralPath ($class+'\InprocServer32')).GetValue('')) $FilterDll)){throw 'Unity Capture registration did not create the two expected owned slots.'}
   Set-Item -LiteralPath $class -Value $slot.Name
   Set-ItemProperty -LiteralPath $instance -Name FriendlyName -Value $slot.Name
  }
 }finally{$lock.Dispose()}
}
function Ensure-BridgeVideoPrerequisites {
 param([ValidateSet('Server','Client')][string]$Role='Client',[string]$RuntimeDirectory=$PSScriptRoot,[string]$FilterDll)
 if(-not [Environment]::Is64BitProcess){throw 'Unity Capture setup requires 64-bit Windows PowerShell.'}
 $runtime=[IO.Path]::GetFullPath($RuntimeDirectory)
 $state=Get-BridgeUnityState $runtime
 # Existing exact slots are reused without executing or relocating their DLL.
 if($state.Ready){return [pscustomobject]@{Ready=$true;Changed=$false;Disposition='ReadyExisting';Filter=$state.Filter;ServerSource='Unity Capture Server';ClientSource='Unity Capture Client'}}
 if($state.Disposition-eq 'Conflict'){throw $state.Reason}
 $manifest=Get-BridgeUnityManifest $runtime
 $source=Join-Path $runtime ([string]$manifest.file).Replace('/','\')
 $null=Assert-BridgeUnityPinnedDll $source $manifest
 $stable=Join-Path (Join-Path (Get-BridgeUnityStableRoot) ([string]$manifest.sha256).ToLowerInvariant()) 'UnityCaptureFilter64.dll'
 $manual=if($FilterDll){Assert-BridgeUnityPinnedDll ([IO.Path]::GetFullPath($FilterDll)) $manifest}else{''}
 foreach($entry in @($state.Entries|Where-Object {$_.Kind-in @('Class','PropertiesClass')})){
  if(-not(Test-BridgeUnitySamePath $entry.Path $stable)-and -not($manual-and (Test-BridgeUnitySamePath $entry.Path $manual))){throw 'A Unity Capture slot belongs to another installation. It was not overwritten. Close video applications and use Configure-UnityCapture.ps1 -FilterDll with that same verified DLL to repair its two slots.'}
 }
 # -FilterDll is explicit consent to repair that same known-hash registration.
 # Still execute only our protected copy, never a DLL from an arbitrary folder.
 $target=Install-BridgeUnityStableDll $source $manifest
 Register-BridgeUnitySlots $target $manifest
 $after=Get-BridgeUnityState $runtime
 if(-not $after.Ready-or -not(Test-BridgeUnitySamePath $after.Filter $target)){throw 'Unity Capture source verification failed after registration. Camera settings were not changed.'}
 [pscustomobject]@{Ready=$true;Changed=$true;Disposition='InstalledOwned';Filter=$target;ServerSource='Unity Capture Server';ClientSource='Unity Capture Client'}
}
if($FunctionsOnly){return}
if(-not [Environment]::Is64BitProcess){throw 'Run this helper in 64-bit Windows PowerShell.'}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 $state=Test-BridgeUnityCaptureReadiness $RuntimeDirectory
 if($state.Ready){[ordered]@{Ready=$true;Changed=$false;Disposition='ReadyExisting';Filter=$state.Filter}|ConvertTo-Json -Compress;exit 0}
 if(-not $Elevate){throw 'Administrator rights are required to prepare the bundled video sources.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Role',$Role,'-RuntimeDirectory',('"'+[IO.Path]::GetFullPath($RuntimeDirectory)+'"'))
 if($FilterDll){$arguments+=@('-FilterDll',('"'+[IO.Path]::GetFullPath($FilterDll)+'"'))}
 $operation=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $operation.WaitForExit();exit $operation.ExitCode
}
try{Ensure-BridgeVideoPrerequisites -Role $Role -RuntimeDirectory $RuntimeDirectory -FilterDll $FilterDll|ConvertTo-Json -Compress;exit 0}catch{
 Add-Type -AssemblyName System.Windows.Forms
 [Windows.Forms.MessageBox]::Show($_.Exception.Message,'USB to Network Bridge video setup','OK','Error')|Out-Null
 exit 1
}
