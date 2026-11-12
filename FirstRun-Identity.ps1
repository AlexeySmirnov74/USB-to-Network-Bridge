. (Join-Path $PSScriptRoot 'Portable-Installation.ps1')
# Dot-source only. No startup, downloads, elevation or configuration writes here.
function ConvertTo-BridgeMachineIdentity([string]$MachineGuid) {
 $value='USBtoNetworkBridge/machine/v1:'+([guid]::Parse($MachineGuid)).ToString('D').ToLowerInvariant()
 $sha=[Security.Cryptography.SHA256]::Create()
 try { -join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($value))|ForEach-Object {$_.ToString('x2')}) } finally {$sha.Dispose()}
}
function Get-BridgeMachineIdentity {
 $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine,[Microsoft.Win32.RegistryView]::Registry64)
 try {
  $key=$base.OpenSubKey('SOFTWARE\Microsoft\Cryptography',$false)
  if(-not $key){throw 'Windows machine identity is unavailable; setup was not changed.'}
  try {$value=[string]$key.GetValue('MachineGuid','')}finally{$key.Dispose()}
  if(-not $value){throw 'Windows machine identity is unavailable; setup was not changed.'}
  ConvertTo-BridgeMachineIdentity $value
 }finally{$base.Dispose()}
}
function Get-BridgeMachineMarkerPath([string]$Role,[string]$Runtime) {
 Join-Path $Runtime ('machine-'+$Role.ToLowerInvariant()+'.local.json')
}
function Read-BridgeJson([string]$Path) {
 if(-not(Test-Path -LiteralPath $Path -PathType Leaf)){return $null}
 $item=Get-Item -LiteralPath $Path
 if($item.Length-gt 1048576){throw 'An installation settings file is too large.'}
 Get-Content -LiteralPath $Path -Raw -Encoding UTF8|ConvertFrom-Json
}
function Write-BridgeJson([string]$Path,$Value) {
 $directory=[IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Path))
 New-Item -ItemType Directory -Path $directory -Force|Out-Null
 $temporary=Join-Path $directory ('.first-run-'+[guid]::NewGuid().ToString('N')+'.tmp')
 try {
  [IO.File]::WriteAllText($temporary,($Value|ConvertTo-Json -Depth 24),[Text.UTF8Encoding]::new($false))
  if(Test-Path -LiteralPath $Path){[IO.File]::Replace($temporary,$Path,[NullString]::Value)}else{[IO.File]::Move($temporary,$Path)}
 }finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Write-BridgeMachineMarker {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Runtime,[string]$Config,[string]$InstallRoot,[switch]$AdoptExisting)
 Write-BridgePortableMachineMarker @PSBoundParameters
}
function Test-BridgeSamePath([string]$First,[string]$Second) {
 if(-not $First-or -not $Second){return $false}
 [string]::Equals([IO.Path]::GetFullPath($First).TrimEnd('\'),[IO.Path]::GetFullPath($Second).TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)
}
function Test-BridgeBroadReplacementPermission($Rules) {
 # Creating a new sibling directory (the normal C:\ ACL) cannot replace an
 # existing protected child. InheritOnly ACEs do not apply to this ancestor.
 $mask=[Security.AccessControl.FileSystemRights]::Delete-bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles-bor [Security.AccessControl.FileSystemRights]::ChangePermissions-bor [Security.AccessControl.FileSystemRights]::TakeOwnership
 foreach($rule in $Rules){
  if($rule.AccessControlType-ne 'Allow'-or ($rule.PropagationFlags-band [Security.AccessControl.PropagationFlags]::InheritOnly)){continue}
  $sid=$rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
  if($sid-in @('S-1-1-0','S-1-5-11','S-1-5-32-545')-and ($rule.FileSystemRights-band $mask)-ne 0){return $true}
 }
 $false
}
function Test-BridgeOwnedRoleTask([string]$Role,[string]$Runtime,[string]$Config) {
 $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName ('USBBridge.'+$Role) -ErrorAction SilentlyContinue
 if(-not $task){return $false}
 $agent=Join-Path $Runtime 'usbbridge-agent.exe'
 $actions=@($task.Actions)
 if($actions.Count-ne 1-or -not(Test-BridgeSamePath $actions[0].Execute $agent)){return $false}
 $match=[regex]::Match([string]$actions[0].Arguments,'(?:^|\s)--config\s+"([^"]+)"(?:\s|$)')
 $mode=if($Role-eq 'Server'){'--host'}else{'--client'}
 $match.Success-and (Test-BridgeSamePath $match.Groups[1].Value $Config)-and ([string]$actions[0].Arguments-match ('(?:^|\s)'+[regex]::Escape($mode)+'(?:\s|$)'))
}
function Test-BridgeOwnedRoleProcess([string]$Role,[string]$Runtime,[string]$Config) {
 $agent=Join-Path $Runtime 'usbbridge-agent.exe'
 foreach($process in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe'" -ErrorAction SilentlyContinue)){
  if(-not(Test-BridgeSamePath $process.ExecutablePath $agent)){continue}
  $match=[regex]::Match([string]$process.CommandLine,'(?:^|\s)--config\s+"([^"]+)"(?:\s|$)')
  if(-not $match.Success-or -not(Test-BridgeSamePath $match.Groups[1].Value $Config)){continue}
  $mode=if($Role-eq 'Server'){'--host'}else{'--client'}
  if([string]$process.CommandLine-match ('(?:^|\s)'+[regex]::Escape($mode)+'(?:\s|$)')){return $true}
 }
 $false
}
function Get-BridgeMachineDisposition {
 param([string]$Role,[string]$Runtime,[string]$Config,[string]$MachineIdentity,[switch]$OwnedTask)
 $marker=Get-BridgeMachineMarker -Role $Role -Runtime $Runtime
 if($marker-and $marker.machine_id){
  if($marker.schema-notin @(1,2)-or $marker.role-ne $Role-or -not $marker.initialized){return 'Unverified'}
  if($marker.machine_id-eq $MachineIdentity){
   if($marker.config-and -not(Test-BridgeSamePath $marker.config $Config)){if($OwnedTask){return 'Adopt'};return 'Unverified'}
   return 'Current'
  }
  return 'Foreign'
 }
 if($OwnedTask-and (Test-Path -LiteralPath $Config -PathType Leaf)){return 'Adopt'}
 if(Test-Path -LiteralPath $Config -PathType Leaf){return 'Unverified'}
 'New'
}
function New-BridgeClientSettings {
 [ordered]@{version=1;server='';server_name='';server_id=$null;proxy=$null;discovery=$true;discovery_port=3241;poll_seconds=3;retry_max_seconds=30;devices=@();servers=@()}
}
function ConvertTo-BridgePortableClientSettings($Settings) {
 # Preserve connection/device selections, never replay machine-bound handles,
 # DPAPI credentials, a previous USB port or a local generated camera source.
 $copy=$Settings|ConvertTo-Json -Depth 24|ConvertFrom-Json
 foreach($connection in @($copy)+@($copy.servers)){
  if(-not $connection){continue}
  if($connection.proxy-and $connection.proxy.PSObject.Properties['credential_file']){$connection.proxy.credential_file=$null}
  foreach($device in @($connection.devices)){if($device.PSObject.Properties['busid']){$device.busid=''}}
 }
 $copy
}
function Get-BridgeExistingLongPath([string]$Path) {
 if(-not('BridgeFirstRunPaths.Native'-as [type])){
  Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
namespace BridgeFirstRunPaths {
 public static class Native {
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true)]
  private static extern uint GetLongPathNameW(string path,StringBuilder result,uint capacity);
  public static string ExistingLongPath(string path) {
   var full=Path.GetFullPath(path);
   var result=new StringBuilder(32768);
   uint length=GetLongPathNameW(full,result,(uint)result.Capacity);
   if(length==0)throw new Win32Exception(Marshal.GetLastWin32Error(),"Cannot identify an existing first-run backup file.");
   if(length>=result.Capacity)throw new PathTooLongException("The first-run backup path exceeds the supported size.");
   return result.ToString();
  }
 }
}
'@
 }
 [BridgeFirstRunPaths.Native]::ExistingLongPath($Path)
}
function Get-BridgeRoleLocalFiles([string]$Role,[string]$Runtime,[string]$Config,[switch]$KeepPreferences) {
 $directory=Split-Path -Parent $Config
 $stem=[IO.Path]::GetFileNameWithoutExtension($Config)
 $prefix=$stem-replace '\.local$',''
 $paths=@($Config,(Get-BridgeMachineMarkerPath $Role $Runtime),(Join-Path $Runtime ('autostart-'+$Role.ToLowerInvariant()+'.local.json')))
 $otherPrefix=if($Role-eq 'Server'){'client'}else{'host'}
 $otherKeys=@()
 foreach($otherDirectory in @($Runtime,(Join-Path $Runtime 'data\Client'),(Join-Path $Runtime 'data\Server'))){
  foreach($otherVideoRole in @('receive','send','local-send')){
   $other=Read-BridgeJson (Join-Path $otherDirectory ($otherPrefix+'.video-'+$otherVideoRole+'.local.json'))
   if($other-and $other.key_file){$otherKeys+=[IO.Path]::GetFileName([string]$other.key_file)}
  }
 }
 foreach($extension in @('status.json','agent.lock','supervisor.lock','edit.lock','catalog.local.json','devices.local.json','proxy-routes.local.json','supervisor.local.json','health.local.json','camera.preferences.json','ui.json','operation.local.json')){$paths+=[IO.Path]::ChangeExtension($Config,$extension)}
 if(-not $KeepPreferences){$paths+=[IO.Path]::ChangeExtension($Config,'preferences.json')}
 foreach($videoRole in @('receive','send','local-send')){
  $video=Join-Path $directory ($prefix+'.video-'+$videoRole+'.local.json')
  $paths+=$video
  foreach($extension in @('status.json','operation.local.json','lock','health.local.json','supervisor.local.json')){$paths+=[IO.Path]::ChangeExtension($video,$extension)}
  $data=Read-BridgeJson $video
  if($data-and $data.key_file){
   $key=Resolve-BridgeConfigPath ([string]$data.key_file) $video $Runtime
   $keyName=[IO.Path]::GetFileName($key)
   # Only archive role-owned local secrets, including the UI's UUID credential
   # files. A copied absolute path is never followed to another installation.
   if($keyName-notin $otherKeys-and [IO.Path]::GetExtension($key)-in @('.key','.credential','.bin','.secret')){
    if((Test-BridgePathWithin $key $directory)-and (Test-Path -LiteralPath $key -PathType Leaf)){$paths+=$key}
    foreach($candidate in @((Join-Path $directory $keyName),(Join-Path (Join-Path $directory 'video-credentials') $keyName))){if(Test-Path -LiteralPath $candidate -PathType Leaf){$paths+=$candidate}}
   }
  }
 }
 if($Role-eq 'Client'){
  $client=Read-BridgeJson $Config
  foreach($connection in @($client)+@($client.servers)){
   if($connection-and $connection.proxy-and $connection.proxy.credential_file){
    $credential=Resolve-BridgeConfigPath ([string]$connection.proxy.credential_file) $Config $Runtime
    if((Test-BridgePathWithin $credential $directory)-and (Test-Path -LiteralPath $credential -PathType Leaf)){$paths+=$credential}
    if(Test-BridgeSamePath ([IO.Path]::GetDirectoryName($credential)) $directory){$paths+=$credential}
    $nested=Join-Path (Join-Path $directory 'proxy-credentials') ([IO.Path]::GetFileName($credential))
    if([IO.Path]::GetExtension($credential)-eq '.credential'-and (Test-Path -LiteralPath $nested -PathType Leaf)){$paths+=$nested}
   }
  }
 }
 if($Role-eq 'Server'){
  $paths+=(Join-Path $directory 'server.local.psd1')
  foreach($file in @('backend-state.json','native-publication.local.json','native-sessions.local.json','native-hid-guards.local.json','device-locations.local.json','devices.inventory','backend-switch-status.json')){$paths+=(Join-Path $Runtime $file)}
 }
 # A copied key can be found by several spellings of the same path. In
 # particular, Windows TEMP may use an 8.3 alias while path resolution expands
 # it. Expand existing names and normalize separators before deduplication so
 # backup moves each file exactly once, preserving link objects as link paths.
 $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 foreach($path in $paths){
  if(Test-Path -LiteralPath $path -PathType Leaf){
   $canonical=Get-BridgeExistingLongPath $path
   if($seen.Add($canonical)){$canonical}
  }
 }
}
function Backup-BridgeRoleState([string]$Role,[string]$Runtime,[string]$Config,[switch]$KeepPreferences) {
 $files=@(Get-BridgeRoleLocalFiles $Role $Runtime $Config -KeepPreferences:$KeepPreferences)
 $backup=$null
 if($files.Count){
  $backup=Join-Path (Split-Path -Parent $Config) ('first-run-backups\'+$Role.ToLowerInvariant()+'-'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')+'-'+[guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $backup -Force|Out-Null
  # Backups may contain raw video keys: never inherit a shared data ACL.
  $acl=New-Object Security.AccessControl.DirectorySecurity
  $acl.SetAccessRuleProtection($true,$false)
  $user=[Security.Principal.WindowsIdentity]::GetCurrent().User
  foreach($sid in @($user,(New-Object Security.Principal.SecurityIdentifier('S-1-5-18')),(New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))){
   $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
  }
  Set-Acl -LiteralPath $backup -AclObject $acl
  $index=0
  foreach($path in $files){Move-Item -LiteralPath $path -Destination (Join-Path $backup ($index.ToString('D3')+'-'+[IO.Path]::GetFileName($path)));$index++}
 }
 $backup
}
function Reset-BridgeRoleState {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Runtime,[string]$Config,[switch]$KeepClientConnections,[string]$ComputerName=$env:COMPUTERNAME)
 $old=if($KeepClientConnections){Read-BridgeJson $Config}else{$null}
 $backup=Backup-BridgeRoleState $Role $Runtime $Config -KeepPreferences:($Role-eq 'Client'-and $KeepClientConnections)
 New-Item -ItemType Directory -Path (Split-Path -Parent $Config) -Force|Out-Null
 if($Role-eq 'Client'){
  $settings=if($old){ConvertTo-BridgePortableClientSettings $old}else{New-BridgeClientSettings}
  Write-BridgeJson $Config $settings
 }else{
  $profile=Join-Path (Split-Path -Parent $Config) 'server.local.psd1'
  [IO.File]::WriteAllText($profile,"@{ Listen='0.0.0.0:3240'; DiscoveryPort=3241; AllowedPeers=@('127.0.0.1'); Export=@(); Debug=`$false; ExpectedInstanceIds=@{} }",[Text.UTF8Encoding]::new($true))
  Write-BridgeJson $Config ([ordered]@{version=1;enabled=$false;server_id=[guid]::NewGuid().ToString();name=$ComputerName;runtime='@app';profile=(ConvertTo-BridgePortablePath $profile $Config $Runtime);allowed_ips=@('127.0.0.1');probe_address='127.0.0.1:3240';discovery_port=3241;auto_remove_unplugged=$true;unplugged_retention_days=1})
  Write-BridgeJson (Join-Path $Runtime 'backend-state.json') ([ordered]@{Backend='Stopped';DesiredBackend='Native';NativePid=$null;NativeStartedUtc=$null;NativeCreatedFileTime=$null;ForcedByUs=@();Reservations=@();PendingStockReleases=@();ExportedBusIds=@()})
 }
 $backup
}
function Get-BridgePublishedVBoxUsbInf([string]$DriverStoreInf) {
 if(-not('BridgeDriverStaging.SetupApi'-as [type])){
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace BridgeDriverStaging {
 public sealed class StagedInf {
  public string StoreInf {get;set;}
  public string PublishedInf {get;set;}
 }
 public static class SetupApi {
  [DllImport("setupapi.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true)]
  [return:MarshalAs(UnmanagedType.Bool)]
  private static extern bool SetupGetInfPublishedNameW(string driverStoreLocation,StringBuilder buffer,uint size,out uint required);
  [DllImport("setupapi.dll",CharSet=CharSet.Unicode,ExactSpelling=true,SetLastError=true)]
  [return:MarshalAs(UnmanagedType.Bool)]
  private static extern bool SetupGetInfDriverStoreLocationW(string file,IntPtr alternatePlatform,string locale,StringBuilder buffer,uint size,out uint required);
  public static StagedInf Resolve(string driverStoreInf) {
   // Resolve the Windows-published OEM INF and round-trip it to its exact
   // staged package. Neither API installs a driver or performs network I/O.
   uint required;
   var published=new StringBuilder(32768);
   if(!SetupGetInfPublishedNameW(driverStoreInf,published,(uint)published.Capacity,out required))return null;
   var store=new StringBuilder(32768);
   if(!SetupGetInfDriverStoreLocationW(published.ToString(),IntPtr.Zero,null,store,(uint)store.Capacity,out required))return null;
   return new StagedInf {StoreInf=store.ToString(),PublishedInf=published.ToString()};
  }
 }
}
'@
 }
 [BridgeDriverStaging.SetupApi]::Resolve($DriverStoreInf)
}
function Test-BridgeStagedVBoxUsbDriver {
 param(
  [string]$MonitorPath,
  [string]$DriverStoreRoot=(Join-Path $env:SystemRoot 'System32\DriverStore\FileRepository'),
  [string]$PublishedInfRoot=(Join-Path $env:SystemRoot 'INF')
 )
 try {
  if(-not $MonitorPath-or [IO.Path]::GetFileName($MonitorPath)-ine 'VBoxUSBMon.sys'){return $false}
  $sourceDirectory=[IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($MonitorPath))
  $expected=@{}
  foreach($name in @('VBoxUSB.inf','VBoxUSB.sys','VBoxUSB.cat')){
   $file=Join-Path $sourceDirectory $name
   if(-not(Test-Path -LiteralPath $file -PathType Leaf)-or (Get-Item -LiteralPath $file).Attributes-band [IO.FileAttributes]::ReparsePoint){return $false}
   $expected[$name]=(Get-FileHash -LiteralPath $file -Algorithm SHA256 -ErrorAction Stop).Hash
  }
  $storeRoot=[IO.Path]::GetFullPath($DriverStoreRoot).TrimEnd('\')
  $publishedRoot=[IO.Path]::GetFullPath($PublishedInfRoot).TrimEnd('\')
  $folders=@(Get-ChildItem -LiteralPath $storeRoot -Directory -Filter 'vboxusb.inf_*' -ErrorAction Stop)
  if($folders.Count-gt 32){return $false}
  foreach($folder in $folders){
   if($folder.Attributes-band [IO.FileAttributes]::ReparsePoint){continue}
   $candidate=Join-Path $folder.FullName 'VBoxUSB.inf'
   if(-not(Test-Path -LiteralPath $candidate -PathType Leaf)-or (Get-Item -LiteralPath $candidate).Attributes-band [IO.FileAttributes]::ReparsePoint){continue}
   if((Get-FileHash -LiteralPath $candidate -Algorithm SHA256 -ErrorAction Stop).Hash-ne $expected['VBoxUSB.inf']){continue}
   $location=Get-BridgePublishedVBoxUsbInf $candidate
   if(-not $location-or -not [string]::Equals([IO.Path]::GetFullPath($location.StoreInf),[IO.Path]::GetFullPath($candidate),[StringComparison]::OrdinalIgnoreCase)){continue}
   $publishedName=[string]$location.PublishedInf
   $published=if([IO.Path]::IsPathRooted($publishedName)){[IO.Path]::GetFullPath($publishedName)}elseif([IO.Path]::GetFileName($publishedName)-ceq $publishedName){Join-Path $publishedRoot $publishedName}else{continue}
   if(-not $published.StartsWith($publishedRoot+'\',[StringComparison]::OrdinalIgnoreCase)-or [IO.Path]::GetFileName($published)-notmatch '^oem[0-9]+\.inf$'-or -not(Test-Path -LiteralPath $published -PathType Leaf)-or (Get-Item -LiteralPath $published).Attributes-band [IO.FileAttributes]::ReparsePoint){continue}
   if((Get-FileHash -LiteralPath $published -Algorithm SHA256 -ErrorAction Stop).Hash-ne $expected['VBoxUSB.inf']){continue}
   $matches=$true
   foreach($name in @('VBoxUSB.sys','VBoxUSB.cat')){
    $file=Join-Path $folder.FullName $name
    if(-not(Test-Path -LiteralPath $file -PathType Leaf)-or (Get-Item -LiteralPath $file).Attributes-band [IO.FileAttributes]::ReparsePoint-or (Get-FileHash -LiteralPath $file -Algorithm SHA256 -ErrorAction Stop).Hash-ne $expected[$name]){$matches=$false;break}
   }
   if($matches){return $true}
  }
 }catch{return $false}
 $false
}
function Get-BridgeDriverReadiness([ValidateSet('Server','Client')][string]$Role,[string]$Runtime) {
 $names=if($Role-eq 'Server'){@('VBoxUSBMon','VBoxUSB')}else{@('usbip2_ude')}
 $missing=@();$present=@();$monitorPath=$null
 $service=if($Role-eq 'Server'){Get-Service usbipd -ErrorAction SilentlyContinue}else{$null}
 foreach($name in $names){
  $driver=Get-CimInstance Win32_SystemDriver -Filter ("Name='"+$name+"'") -ErrorAction SilentlyContinue
  if(-not $driver){
   # usbipd pre-stages the device INF. VBoxUSB's service is created only when
   # a USB device first binds to it; absence of that service is normal beforehand.
   if($name-eq 'VBoxUSB'-and $monitorPath-and $service-and (Test-BridgeStagedVBoxUsbDriver $monitorPath)){$present+=$name}else{$missing+=$name}
   continue
  }
  $present+=$name
  $path=[Environment]::ExpandEnvironmentVariables(([string]$driver.PathName).Trim('"'))
  $path=$path-replace '^\\SystemRoot\\',($env:SystemRoot+'\')
  $path=$path-replace '^\\\?\?\\',''
  if(-not $path-or -not(Test-Path -LiteralPath $path -PathType Leaf)-or $driver.StartMode-eq 'Disabled'){$missing+=$name}
  elseif($name-eq 'VBoxUSBMon'){$monitorPath=$path}
 }
 if($Role-eq 'Server'-and -not $service){$missing+='usbipd'}
 if($Role-eq 'Client'-and $missing.Count-eq 0-and $Runtime){
  $probe=Join-Path $Runtime 'usbbridge-client.exe'
  if(-not(Test-Path -LiteralPath $probe -PathType Leaf)){$missing+='client probe'}else{
   & $probe ports *> $null
   if($LASTEXITCODE-ne 0){$missing+='virtual controller interface'}
  }
 }
 [pscustomobject]@{Ready=($missing.Count-eq 0);Missing=@($missing);Partial=($present.Count-gt 0-or [bool]$service)}
}
function Get-BridgeInstallationReadiness([ValidateSet('Server','Client')][string]$Role,[string]$Runtime) {
 # Keep the physical-driver probe separate: a missing video source must not
 # trigger a USB controller repair or interrupt existing USB sessions.
 $drivers=Get-BridgeDriverReadiness $Role $Runtime
 $missing=@($drivers.Missing)
 $videoReady=$Role-ne 'Client'
 if($Role-eq 'Client'){
  $helper=Join-Path $Runtime 'Ensure-VideoPrerequisites.ps1'
  if(-not(Test-Path -LiteralPath $helper -PathType Leaf)){$missing+='bundled video setup'}else{
   . $helper -Role $Role -RuntimeDirectory $Runtime -FunctionsOnly
   $video=Test-BridgeUnityCaptureReadiness $Runtime
   $videoReady=[bool]$video.Ready
   if(-not $video.Ready){$missing+='Unity Capture Server/Client sources'}
  }
 }
 [pscustomobject]@{Ready=($missing.Count-eq 0);Missing=@($missing);Partial=$drivers.Partial;DriversReady=[bool]$drivers.Ready;VideoReady=$videoReady}
}
function Repair-BridgeExistingVideoPrerequisites {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Disposition,[string]$Config,[string]$Runtime,$Readiness)
 if($Role-ne 'Client'-or $Disposition-notin @('Current','Adopt')-or -not(Test-Path -LiteralPath $Config -PathType Leaf)-or -not $Readiness.DriversReady-or $Readiness.VideoReady){return $Readiness}
 # This is a component-only upgrade of an already verified installation.
 # Do not enter role setup: its data ACLs, keys and startup tasks are unrelated.
 & (Join-Path $Runtime 'Ensure-VideoPrerequisites.ps1') -Role Client -RuntimeDirectory $Runtime -Elevate|Out-Null
 if(-not $?){throw 'The local Unity Capture sources could not be prepared. Existing role settings and startup were preserved.'}
 $after=Get-BridgeInstallationReadiness $Role $Runtime
 if(-not $after.VideoReady){throw 'Unity Capture sources are still unavailable after local setup. Existing role settings and startup were preserved.'}
 $after
}
