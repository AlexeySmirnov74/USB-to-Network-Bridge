# Rebind one verified, already configured installation after its directory moved.
# Configuration, camera serials and keys remain untouched. Windows registration
# is intentionally stored outside the portable directory.
[CmdletBinding()]
param([string]$InstallRoot,[string]$DeploymentId,[switch]$Elevate,[switch]$FunctionsOnly)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
function Get-RelocationCanonicalPath([string]$Value){
 if(-not $Value-or -not [IO.Path]::IsPathRooted($Value)){throw 'An absolute Windows registration path is required.'}
 [IO.Path]::GetFullPath($Value).TrimEnd('\')
}
function Assert-RelocationTree([string]$Directory){
 $current=Get-Item -LiteralPath $Directory -Force
 while($current){
  if($current.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'An installation path must not pass through a symbolic link or junction.'}
  $current=$current.Parent
 }
}
function Resolve-RelocationBindingConfig([string]$Value,[string]$Runtime){
 if(-not $Value){return ''}
 if([IO.Path]::IsPathRooted($Value)){return Get-RelocationCanonicalPath $Value}
 if($Value-eq '@app'){return Get-RelocationCanonicalPath $Runtime}
 if($Value.StartsWith('@app/',[StringComparison]::OrdinalIgnoreCase)-or $Value.StartsWith('@app\',[StringComparison]::OrdinalIgnoreCase)){$Value=$Value.Substring(5)}
 elseif($Value.StartsWith('@programdata/',[StringComparison]::OrdinalIgnoreCase)-or $Value.StartsWith('@programdata\',[StringComparison]::OrdinalIgnoreCase)){
  $base=[Environment]::GetFolderPath('CommonApplicationData')
  $path=Get-RelocationCanonicalPath (Join-Path $base $Value.Substring(13))
  if(-not $path.StartsWith(($base.TrimEnd('\')+'\'),[StringComparison]::OrdinalIgnoreCase)){throw 'A registered configuration escaped its data directory.'}
  return $path
 }
 elseif($Value.StartsWith('@')){throw 'An unknown configuration locator was registered.'}
 $path=Get-RelocationCanonicalPath (Join-Path $Runtime $Value)
 if(-not $path.StartsWith(($Runtime.TrimEnd('\')+'\'),[StringComparison]::OrdinalIgnoreCase)){throw 'A registered configuration escaped its runtime directory.'}
 $path
}
function Get-RelocationAllowedConfigs($Binding,[string]$Runtime){
 $values=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 foreach($role in @('Server','Client')){
  $value=Get-RelocationRoleConfig $Binding $role $Runtime
  if(-not $value){continue}
  [void]$values.Add($value)
  $stem=[IO.Path]::GetFileNameWithoutExtension($value)-replace '\.local$',''
  foreach($video in @('receive','send','local-send')){[void]$values.Add((Join-Path ([IO.Path]::GetDirectoryName($value)) ($stem+'.video-'+$video+'.local.json')))}
 }
 # Return a hash set as one object; PowerShell otherwise unwraps its elements.
 ,$values
}
function Get-RelocationRoleConfig($Binding,[string]$Role,[string]$Runtime){
 $value=[string]$Binding.($Role+'Config')
 if($value-eq '@external'){return Get-RelocationCanonicalPath ([string]$Binding.($Role+'ExternalConfig'))}
 Resolve-RelocationBindingConfig $value $Runtime
}
function Test-RelocationRoleMarker($Marker,[string]$Role,[string]$Machine,[string]$Deployment,$Binding){
 # A copied folder may still contain the other role's marker from the previous
 # PC. Only initialized roles registered to this machine belong to this move.
 if(-not $Marker-or $Marker.machine_id-ne $Machine-or -not [string]$Binding.($Role+'Config')){return $false}
 if($Marker.schema-ne 2-or $Marker.deployment_id-ne $Deployment-or $Marker.role-ne $Role-or -not $Marker.initialized){throw 'A role marker does not match this machine and deployment.'}
 $true
}
function Assert-RelocationOwnedTask([string]$TaskXml,[string]$OldRuntime,$AllowedConfigs){
 $exec=Get-MaintenanceExec $TaskXml
 if([string]$exec.Command-notin @((Join-Path $OldRuntime 'usbbridge-agent.exe'),(Join-Path $OldRuntime 'usbbridge-camera.exe'))){throw 'The startup task executable belongs to another installation.'}
 $config=Get-MaintenanceConfigArgument ([string]$exec.Arguments)
 if(-not $config-or -not $AllowedConfigs.Contains($config)){throw 'The startup task configuration belongs to another installation.'}
 if([string]$exec.Command-eq (Join-Path $OldRuntime 'usbbridge-camera.exe')){
  if([string]$exec.Arguments-notmatch '^(?:receive|send|check-receive|check-send|check-local-send)\s+--config\s+'){throw 'Unknown video startup task action.'}
 }elseif([string]$exec.Arguments-notmatch '(?:^|\s)(?:--host|--client|--role\s+(?:server|client))(?:\s|$)'){throw 'Unknown USB Bridge startup role.'}
 $config
}
function Get-RelocationShortcutSnapshot([string]$OldRoot,[string]$OldRuntime){
 $results=New-Object 'Collections.Generic.List[object]';$shell=New-Object -ComObject WScript.Shell
 foreach($directory in @([Environment]::GetFolderPath('CommonStartMenu'),[Environment]::GetFolderPath('StartMenu'),[Environment]::GetFolderPath('DesktopDirectory'),[Environment]::GetFolderPath('CommonDesktopDirectory'))|Select-Object -Unique){
  if(-not $directory-or -not(Test-Path -LiteralPath $directory)){continue}
  foreach($file in @(Get-ChildItem -LiteralPath $directory -Filter '*.lnk' -Recurse -File -ErrorAction SilentlyContinue)){
   $link=$shell.CreateShortcut($file.FullName)
   if($link.TargetPath-in @((Join-Path $OldRoot 'USB-to-Network-Bridge-Client.exe'),(Join-Path $OldRoot 'USB-to-Network-Bridge-Server.exe'),(Join-Path $OldRuntime 'usbbridge-agent.exe'))){
    $results.Add([pscustomobject]@{path=$file.FullName;target=$link.TargetPath;arguments=$link.Arguments;working_directory=$link.WorkingDirectory;icon=$link.IconLocation})
   }
  }
 }
 @($results.ToArray())
}
function Set-RelocationShortcut($Entry,[string]$OldRoot,[string]$NewRoot,[string]$OldRuntime,[string]$NewRuntime,[switch]$Restore){
 $shell=New-Object -ComObject WScript.Shell;$link=$shell.CreateShortcut($Entry.path)
 if($Restore){$link.TargetPath=$Entry.target;$link.Arguments=$Entry.arguments;$link.WorkingDirectory=$Entry.working_directory;$link.IconLocation=$Entry.icon}
 else{
  $link.TargetPath=Convert-MaintenancePath ([string]$Entry.target) $OldRuntime $NewRuntime
  $link.TargetPath=Convert-MaintenancePath ([string]$link.TargetPath) $OldRoot $NewRoot
  $link.Arguments=Convert-MaintenanceArguments ([string]$Entry.arguments) $OldRuntime $NewRuntime
  $link.WorkingDirectory=Convert-MaintenancePath ([string]$Entry.working_directory) $OldRuntime $NewRuntime
  $link.WorkingDirectory=Convert-MaintenancePath ([string]$link.WorkingDirectory) $OldRoot $NewRoot
  if($Entry.icon-match '^(?<path>.+?)(?<index>,-?\d+)?$'){$link.IconLocation=(Convert-MaintenancePath (Convert-MaintenancePath $Matches.path $OldRuntime $NewRuntime) $OldRoot $NewRoot)+$Matches.index}
 }
 $link.Save()
}
function Write-RelocationJournal([string]$Path,$Journal){
 $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
 try{[IO.File]::WriteAllText($temporary,($Journal|ConvertTo-Json -Depth 32),[Text.UTF8Encoding]::new($false));Move-Item -LiteralPath $temporary -Destination $Path -Force}
 finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Assert-RelocationRecovery($Journal,[string]$Deployment,[string]$Machine,[string]$OldRoot,[string]$OldRuntime,[string]$NewRoot,[string]$NewRuntime,$AllowedConfigs){
 if($Journal.schema-ne 1-or $Journal.deployment_id-ne $Deployment-or $Journal.machine_id-ne $Machine-or $Journal.old_root-ne $OldRoot-or $Journal.old_runtime-ne $OldRuntime-or $Journal.new_root-ne $NewRoot-or $Journal.new_runtime-ne $NewRuntime){throw 'The protected recovery journal belongs to a different relocation.'}
 foreach($entry in @($Journal.tasks)){
  if($entry.path-ne '\USBBridge\'-or -not $entry.name){throw 'Unexpected task in the relocation recovery journal.'}
  $null=Assert-RelocationOwnedTask $entry.xml $OldRuntime $AllowedConfigs
 }
 $programs=@((Join-Path $OldRuntime 'usbbridge-agent.exe'),(Join-Path $OldRuntime 'usbbridge-camera.exe'),(Join-Path $OldRuntime 'usbbridge-server.exe'))
 foreach($entry in @($Journal.firewall)){if($entry.name-notlike 'USBBridge-*'-or $entry.program-notin $programs){throw 'Unexpected firewall ownership in the relocation recovery journal.'}}
}
function Stop-RelocationNative([string]$OldRuntime,[string]$NewRuntime){
 $statePath=Join-Path $NewRuntime 'backend-state.json'
 if(-not(Test-Path -LiteralPath $statePath -PathType Leaf)){return}
 $state=Get-Content -LiteralPath $statePath -Raw -Encoding UTF8|ConvertFrom-Json
 if(-not $state.NativePid){return}
 $process=Get-Process -Id ([int]$state.NativePid) -ErrorAction SilentlyContinue
 if(-not $process){return}
 try{
  $null=$process.Handle
  if($process.Path-notin @((Join-Path $OldRuntime 'usbbridge-server.exe'),(Join-Path $NewRuntime 'usbbridge-server.exe'))){throw 'The recorded native process belongs to another installation.'}
  if(-not $state.NativeCreatedFileTime-or $process.StartTime.ToFileTimeUtc().ToString()-ne [string]$state.NativeCreatedFileTime){throw 'The recorded native process identity has changed.'}
  $process.Kill();if(-not $process.WaitForExit(15000)){throw 'The owned native USB transport did not stop.'}
 }finally{$process.Dispose()}
}
if($FunctionsOnly){return}
if(-not $InstallRoot-or -not $DeploymentId){throw 'InstallRoot and DeploymentId are required.'}
$deployment=([guid]::Parse($DeploymentId)).ToString('D').ToLowerInvariant()
$root=Get-RelocationCanonicalPath $InstallRoot;$runtime=Get-RelocationCanonicalPath $PSScriptRoot
if($runtime-ne (Join-Path $root 'app')){throw 'The relocation helper must run from this installation app directory.'}
Assert-RelocationTree $root
foreach($name in @('USB-to-Network-Bridge-Client.exe','USB-to-Network-Bridge-Server.exe')){if(-not(Test-Path -LiteralPath (Join-Path $root $name) -PathType Leaf)){throw 'The moved directory has no complete pair of launchers.'}}
foreach($name in @('usbbridge-agent.exe','usbbridge-camera.exe','usbbridge-server.exe','FirstRun-Identity.ps1','Maintain-Installation.ps1','New-BackendMutex.ps1','Portable-Paths.ps1','Portable-Installation.ps1')){if(-not(Test-Path -LiteralPath (Join-Path $runtime $name) -PathType Leaf)){throw ('The moved application is missing '+$name)}}
. (Join-Path $runtime 'FirstRun-Identity.ps1')
. (Join-Path $runtime 'Maintain-Installation.ps1') -FunctionsOnly
. (Join-Path $runtime 'New-BackendMutex.ps1')
. (Join-Path $runtime 'Portable-Paths.ps1')
. (Join-Path $runtime 'Portable-Installation.ps1')
$machine=Get-BridgeMachineIdentity
$registration='HKLM:\Software\USBtoNetworkBridge\Deployments\'+$deployment
$binding=Get-ItemProperty -LiteralPath $registration -ErrorAction Stop
if($binding.MachineId-ne $machine){throw 'This deployment belongs to another Windows installation; use first-run setup.'}
$oldRuntime=Get-RelocationCanonicalPath ([string]$binding.RuntimeDirectory)
$oldRoot=Get-RelocationCanonicalPath ([string]$binding.InstallRoot)
if($oldRuntime-ne (Join-Path $oldRoot 'app')){throw 'The protected deployment registration has an invalid runtime.'}
$stamp=Read-BridgeJson (Join-Path $runtime 'deployment.local.json')
if(-not $stamp-or ([guid]::Parse([string]$stamp.deployment_id)).ToString('D')-ne $deployment){throw 'The moved directory does not match the protected deployment registration.'}
$markers=@();foreach($role in @('Server','Client')){
 $marker=Read-BridgeJson (Get-BridgeMachineMarkerPath $role $runtime)
 if(Test-RelocationRoleMarker $marker $role $machine $deployment $binding){$markers+=@($marker)}
}
if(-not $markers.Count){throw 'No initialized role belongs to this deployment.'}
if($oldRuntime-eq $runtime-and $oldRoot-eq $root-and -not $binding.RelocationPending){return}
$administrator=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $administrator){
 if(-not $Elevate){throw 'Moving a configured installation requires elevation; pass -Elevate for UAC.'}
 $powershell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-InstallRoot',('"'+$root+'"'),'-DeploymentId',$deployment)
 $child=Start-Process -FilePath $powershell -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru;$child.WaitForExit()
 if($child.ExitCode-ne 0){throw 'The moved installation could not be rebound. Its recovery journal was retained.'}
 return
}
$journalDirectory=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('USB to Network Bridge\Relocation\'+$deployment)
$pendingPath=Join-Path $journalDirectory 'pending.local.json'
$pending=if(Test-Path -LiteralPath $pendingPath -PathType Leaf){Read-BridgeJson $pendingPath}else{$null}
if($pending){
 if($pending.deployment_id-ne $deployment-or $pending.machine_id-ne $machine-or $pending.new_root-ne $root-or $pending.new_runtime-ne $runtime-or $oldRuntime-notin @($pending.old_runtime,$pending.new_runtime)-or $oldRoot-notin @($pending.old_root,$pending.new_root)){throw 'A different relocation owns the protected recovery journal.'}
 $oldRoot=Get-RelocationCanonicalPath ([string]$pending.old_root);$oldRuntime=Get-RelocationCanonicalPath ([string]$pending.old_runtime)
}
if($oldRuntime-eq $runtime-and $oldRoot-eq $root-and -not $pending){Remove-ItemProperty -LiteralPath $registration -Name RelocationPending -ErrorAction SilentlyContinue;return}
# Recheck the complete payload against the last trusted, protected manifest.
Assert-BridgeRelocatedRuntime -InstallRoot $root -Runtime $runtime -Binding $binding
$locks=New-Object 'Collections.Generic.List[object]';$journal=$null;$journalPath=$null;$changed=$false
try{
 foreach($location in @($oldRuntime,$runtime)|Sort-Object -Unique){
  $mutex=New-BridgeBackendMutex $location;$held=$false
  try{try{$held=$mutex.WaitOne(45000)}catch [Threading.AbandonedMutexException]{$held=$true};if(-not $held){throw 'Another backend operation is still in progress.'};$locks.Add($mutex)}catch{$mutex.Dispose();throw}
 }
 # All discovery and ownership validation precedes task/process mutations.
 $currentBinding=Get-ItemProperty -LiteralPath $registration
 if($currentBinding.MachineId-ne $machine-or $currentBinding.RuntimeDirectory-notin @($oldRuntime,$runtime)-or $currentBinding.InstallRoot-notin @($oldRoot,$root)){throw 'The protected deployment registration changed during relocation.'}
 $allowed=Get-RelocationAllowedConfigs $binding $oldRuntime
 foreach($marker in $markers){
  $roleConfig=Get-RelocationRoleConfig $binding $marker.role $oldRuntime
  if(-not $roleConfig-or -not(Test-Path -LiteralPath (Convert-MaintenancePath $roleConfig $oldRuntime $runtime) -PathType Leaf)){throw 'The moved deployment is missing its registered role configuration.'}
 }
 $programs=@((Join-Path $oldRuntime 'usbbridge-agent.exe'),(Join-Path $oldRuntime 'usbbridge-camera.exe'))
 $tasks=New-Object 'Collections.Generic.List[object]'
 foreach($task in @(Get-ScheduledTask -ErrorAction Stop|Where-Object {@($_.Actions|Where-Object {$_.Execute-in $programs}).Count})){
  if($task.TaskPath-ne '\USBBridge\'){throw 'An unexpected startup task points to this installation.'}
  $xml=Export-ScheduledTask -TaskPath $task.TaskPath -TaskName $task.TaskName
  $config=Assert-RelocationOwnedTask $xml $oldRuntime $allowed
  $newConfig=Convert-MaintenancePath $config $oldRuntime $runtime
  if(-not(Test-Path -LiteralPath $newConfig -PathType Leaf)){throw 'An owned startup configuration is missing from the moved installation.'}
  $tasks.Add([pscustomobject]@{path=$task.TaskPath;name=$task.TaskName;xml=$xml;new_xml=(Convert-MaintenanceTaskXml $xml $oldRuntime $runtime);running=($task.State-eq 'Running');enabled=(Test-MaintenanceTaskEnabled $xml);config=$newConfig})
 }
 $firewall=New-Object 'Collections.Generic.List[object]'
 foreach($rule in @(Get-NetFirewallRule -Name 'USBBridge-*' -ErrorAction SilentlyContinue)){
  $app=$rule|Get-NetFirewallApplicationFilter
  if($app.Program-in ($programs+@((Join-Path $oldRuntime 'usbbridge-server.exe')))){$firewall.Add([pscustomobject]@{name=$rule.Name;program=$app.Program;new_program=(Convert-MaintenancePath $app.Program $oldRuntime $runtime)})}
 }
 $shortcuts=@(Get-RelocationShortcutSnapshot $oldRoot $oldRuntime)
 $legacyRegistry=@();$legacyBinding=Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -ErrorAction SilentlyContinue
 foreach($marker in $markers){
  $property=$marker.role+'Directory';$value=[string]$legacyBinding.$property
  if($value-in @($oldRoot,$oldRuntime)){$legacyRegistry+=@([pscustomobject]@{property=$property;value=$value;new_value=$(if($value-eq $oldRuntime){$runtime}else{$root})})}
 }
 New-Item -ItemType Directory -Path $journalDirectory -Force|Out-Null
 Assert-RelocationTree $journalDirectory
 $acl=New-Object Security.AccessControl.DirectorySecurity;$acl.SetAccessRuleProtection($true,$false)
 foreach($id in @('S-1-5-18','S-1-5-32-544')){$sid=New-Object Security.Principal.SecurityIdentifier($id);$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))}
 Set-Acl -LiteralPath $journalDirectory -AclObject $acl
 $journalPath=Join-Path $journalDirectory 'pending.local.json'
 if(Test-Path -LiteralPath $journalPath){
  $previous=Read-BridgeJson $journalPath
  Assert-RelocationRecovery $previous $deployment $machine $oldRoot $oldRuntime $root $runtime $allowed
  $tasks.Clear()
  foreach($entry in @($previous.tasks)){
   $existing=Get-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction Stop
   $current=Get-MaintenanceExec (Export-ScheduledTask -TaskPath $entry.path -TaskName $entry.name)
   if([string]$current.Command-notin @((Join-Path $oldRuntime 'usbbridge-agent.exe'),(Join-Path $oldRuntime 'usbbridge-camera.exe'),(Join-Path $runtime 'usbbridge-agent.exe'),(Join-Path $runtime 'usbbridge-camera.exe'))){throw 'Another installation now owns a recovery task.'}
   $oldConfig=Assert-RelocationOwnedTask $entry.xml $oldRuntime $allowed;$newConfig=Convert-MaintenancePath $oldConfig $oldRuntime $runtime
   if((Get-MaintenanceConfigArgument ([string]$current.Arguments))-notin @($oldConfig,$newConfig)){throw 'Another configuration now owns a recovery task.'}
   $entry.new_xml=Convert-MaintenanceTaskXml $entry.xml $oldRuntime $runtime;$entry.config=$newConfig;$tasks.Add($entry)
  }
  $firewall.Clear();foreach($entry in @($previous.firewall)){
   $entry.new_program=Convert-MaintenancePath $entry.program $oldRuntime $runtime
   $current=Get-NetFirewallRule -Name $entry.name -ErrorAction Stop|Get-NetFirewallApplicationFilter
   if($current.Program-notin @($entry.program,$entry.new_program)){throw 'Another installation now owns a recovery firewall rule.'}
   $firewall.Add($entry)
  }
  $shortcuts=@($previous.shortcuts);$journal=$previous;$journal.phase='prepared'
  $legacyRegistry=@($previous.legacy_registry)
 }else{$journal=[ordered]@{schema=1;deployment_id=$deployment;machine_id=$machine;old_root=$oldRoot;old_runtime=$oldRuntime;new_root=$root;new_runtime=$runtime;tasks=@($tasks.ToArray());firewall=@($firewall.ToArray());shortcuts=$shortcuts;legacy_registry=$legacyRegistry;phase='prepared';updated_utc=[DateTime]::UtcNow.ToString('o')}}
 Write-RelocationJournal $journalPath $journal;$changed=$true
 New-ItemProperty -LiteralPath $registration -Name RelocationPending -PropertyType DWord -Value 1 -Force|Out-Null
 foreach($entry in $tasks){Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}
 # Release only the generated receivers explicitly registered by this deployment.
 foreach($entry in $tasks){
  $exec=Get-MaintenanceExec $entry.xml
  if([string]$exec.Command-ne (Join-Path $oldRuntime 'usbbridge-camera.exe')-or [string]$exec.Arguments-notmatch '^receive\s+--config\s+'){continue}
  $video=Read-BridgeJson $entry.config
  $port=[uint16]$video.usb_port;if(-not $port){throw 'The owned generated receiver has an invalid USB port.'}
  & (Join-Path $runtime 'usbbridge-camera.exe') detach-local ('127.0.0.1:'+$port)|Out-Null
  if($LASTEXITCODE-ne 0){throw 'The owned generated camera could not be released.'}
 }
 foreach($entry in $tasks){Stop-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue}
 $newAllowed=Get-RelocationAllowedConfigs $binding $runtime
 foreach($item in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe' OR Name='usbbridge-camera.exe'")){
  if($item.ExecutablePath-notin ($programs+@((Join-Path $runtime 'usbbridge-agent.exe'),(Join-Path $runtime 'usbbridge-camera.exe')))){continue}
  $config=Get-MaintenanceConfigArgument ([string]$item.CommandLine)
  if(-not $config-or (-not $allowed.Contains($config)-and -not $newAllowed.Contains($config))){continue}
  $process=Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue
  if($process){try{$null=$process.Handle;if(-not $process.HasExited){if($process.Path-ne $item.ExecutablePath){throw 'An owned process identity changed during relocation.'};$process.Kill();if(-not $process.WaitForExit(15000)){throw 'An owned USB Bridge process did not stop.'}}}finally{$process.Dispose()}}
 }
 if(@($markers|Where-Object role -eq 'Server').Count){
  Stop-RelocationNative $oldRuntime $runtime
  $hostConfig=Convert-MaintenancePath (Get-RelocationRoleConfig $binding 'Server' $oldRuntime) $oldRuntime $runtime
  $hostData=Resolve-BridgeConfigPaths -Value (Read-BridgeJson $hostConfig) -Config $hostConfig -RuntimeDirectory $runtime
  $state=Read-BridgeJson (Join-Path $runtime 'backend-state.json')
  if($state-and $state.DesiredBackend){& (Join-Path $runtime 'Switch-Backend.ps1') -Backend $state.DesiredBackend -RuntimeDirectory $runtime -Profile $hostData.profile -Stop;if(-not $?){throw 'The owned physical USB captures could not be released.'}}
 }
 foreach($entry in $tasks){
  $current=Get-MaintenanceExec (Export-ScheduledTask -TaskPath $entry.path -TaskName $entry.name)
  $original=Get-MaintenanceExec $entry.xml;$replacement=Get-MaintenanceExec $entry.new_xml
  if([string]$current.Command-notin @([string]$original.Command,[string]$replacement.Command)-or (Get-MaintenanceConfigArgument ([string]$current.Arguments))-notin @((Get-MaintenanceConfigArgument ([string]$original.Arguments)),(Get-MaintenanceConfigArgument ([string]$replacement.Arguments)))){throw 'A different installation changed a startup task during relocation.'}
  Register-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -Xml (Set-MaintenanceTaskEnabledXml $entry.new_xml $false) -Force|Out-Null
 }
 foreach($entry in $firewall){
  $current=Get-NetFirewallRule -Name $entry.name|Get-NetFirewallApplicationFilter
  if($current.Program-notin @($entry.program,$entry.new_program)){throw 'A different installation changed a firewall rule during relocation.'}
  $current|Set-NetFirewallApplicationFilter -Program $entry.new_program|Out-Null
 }
 foreach($entry in $shortcuts){Set-RelocationShortcut $entry $oldRoot $root $oldRuntime $runtime}
 foreach($entry in $legacyRegistry){
  $current=[string](Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge').($entry.property)
  if($current-notin @($entry.value,$entry.new_value)){throw 'A different installation now owns the legacy role registration.'}
  Set-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -Name $entry.property -Value $entry.new_value
 }
 Set-ItemProperty -LiteralPath $registration -Name InstallRoot -Value $root
 Set-ItemProperty -LiteralPath $registration -Name RuntimeDirectory -Value $runtime
 $journal.phase='rebound';Write-RelocationJournal $journalPath $journal
 foreach($entry in $tasks){if($entry.enabled){Enable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null};if($entry.running-and $entry.enabled){Start-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}}
 Remove-Item -LiteralPath $journalPath -Force;Remove-ItemProperty -LiteralPath $registration -Name RelocationPending -ErrorAction SilentlyContinue;$changed=$false
}catch{
 $failure=$_
 if($changed-and $journal){
  # Keep disabled old-path task definitions when the original directory no longer
  # exists. The protected journal retains the exact previous running state.
  foreach($entry in @($journal.tasks)){try{
   $current=Get-MaintenanceExec (Export-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction Stop)
   $original=Get-MaintenanceExec $entry.xml;$replacement=Get-MaintenanceExec $entry.new_xml
   if([string]$current.Command-notin @([string]$original.Command,[string]$replacement.Command)-or (Get-MaintenanceConfigArgument ([string]$current.Arguments))-notin @((Get-MaintenanceConfigArgument ([string]$original.Arguments)),(Get-MaintenanceConfigArgument ([string]$replacement.Arguments)))){continue}
   Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue|Out-Null;Stop-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue;Register-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -Xml (Set-MaintenanceTaskEnabledXml $entry.xml $false) -Force|Out-Null
  }catch{}}
  foreach($entry in @($journal.firewall)){try{$current=Get-NetFirewallRule -Name $entry.name|Get-NetFirewallApplicationFilter;if($current.Program-in @($entry.program,$entry.new_program)){$current|Set-NetFirewallApplicationFilter -Program $entry.program|Out-Null}}catch{}}
  foreach($entry in @($journal.shortcuts)){try{Set-RelocationShortcut $entry $oldRoot $root $oldRuntime $runtime -Restore}catch{}}
  foreach($entry in @($journal.legacy_registry)){try{$current=[string](Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge').($entry.property);if($current-in @($entry.value,$entry.new_value)){Set-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -Name $entry.property -Value $entry.value}}catch{}}
  try{Set-ItemProperty -LiteralPath $registration -Name InstallRoot -Value $oldRoot;Set-ItemProperty -LiteralPath $registration -Name RuntimeDirectory -Value $oldRuntime}catch{}
  if(Test-Path -LiteralPath (Join-Path $oldRuntime 'usbbridge-agent.exe') -PathType Leaf){foreach($entry in @($journal.tasks)){try{
   $current=Get-MaintenanceExec (Export-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction Stop)
   $original=Get-MaintenanceExec $entry.xml
   if([string]$current.Command-ne [string]$original.Command-or (Get-MaintenanceConfigArgument ([string]$current.Arguments))-ne (Get-MaintenanceConfigArgument ([string]$original.Arguments))){continue}
   if($entry.enabled){Enable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null};if($entry.running-and $entry.enabled){Start-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}
  }catch{}}}
  $journal.phase='rollback_retained';try{Write-RelocationJournal $journalPath $journal}catch{}
 }
 throw $failure
}finally{
 for($index=$locks.Count-1;$index-ge 0;$index--){try{$locks[$index].ReleaseMutex()}finally{$locks[$index].Dispose()}}
}
