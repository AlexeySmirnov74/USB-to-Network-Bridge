# First-run setup for the MSI-installed application. Drivers are never embedded.
param([Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,[switch]$Elevate)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$runtime=$PSScriptRoot
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'First-run setup requires Administrator rights.'}
 $p=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Role',$Role -PassThru
 $p.WaitForExit();exit $p.ExitCode
}
try {
 $driverResult=@()
 $base=[Environment]::GetFolderPath('CommonApplicationData')
 $data=Join-Path $base 'USB to Network Bridge'
 $roleData=Join-Path $data $Role
 $config=Join-Path $roleData $(if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'})
 $agent=Join-Path $runtime 'usbbridge-agent.exe'
 # Refuse to redirect another installed/portable controller.
 $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName ('USBBridge.'+$Role) -ErrorAction SilentlyContinue
 if($task-and @($task.Actions|Where-Object Execute -ne $agent).Count){throw 'Another installation owns this role. Migrate or disable its startup before configuring this installation.'}
 # Code/helpers must be protected from unprivileged modification before SYSTEM tasks are created.
 $current=Get-Item -LiteralPath $runtime
 while($current){
  if($current.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'The installed application path must not contain a junction or symlink.'}
  $current=$current.Parent
 }
 $writeMask=[Security.AccessControl.FileSystemRights]::Write-bor [Security.AccessControl.FileSystemRights]::Delete-bor [Security.AccessControl.FileSystemRights]::ChangePermissions-bor [Security.AccessControl.FileSystemRights]::TakeOwnership
 foreach($rule in (Get-Acl -LiteralPath $runtime).Access){
  $sid=$rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
  if($rule.AccessControlType-eq 'Allow'-and $sid-in @('S-1-1-0','S-1-5-11','S-1-5-32-545')-and ($rule.FileSystemRights-band $writeMask)-ne 0){throw 'The application directory allows unprivileged writes; choose a protected Program Files installation.'}
 }
 function Protect-Directory([string]$Path,[bool]$UserWrite){
  New-Item -ItemType Directory -Path $Path -Force|Out-Null
  if((Get-Item -LiteralPath $Path).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Configuration directory must not be a reparse point.'}
  $acl=New-Object Security.AccessControl.DirectorySecurity
  $acl.SetAccessRuleProtection($true,$false)
  foreach($entry in @(@('S-1-5-18','FullControl'),@('S-1-5-32-544','FullControl'),@('S-1-5-32-545',$(if($UserWrite){'Modify'}else{'ReadAndExecute'})))){
   $sid=New-Object Security.Principal.SecurityIdentifier($entry[0])
   $rule=New-Object Security.AccessControl.FileSystemAccessRule($sid,$entry[1],'ContainerInherit,ObjectInherit','None','Allow')
   $acl.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $Path -AclObject $acl
 }
 Protect-Directory $data $false
 Protect-Directory $roleData ($Role-eq 'Client')
 if($Role-eq 'Server'){
  if(-not(Get-Service usbipd -ErrorAction SilentlyContinue)){$driverResult=@(& (Join-Path $runtime 'Install-ProductionDrivers.ps1') -Role Server -AllowedPeer 127.0.0.1);if(-not $?){throw 'Server driver installation failed.'}}
  $profile=Join-Path $roleData 'server.local.psd1'
  if(-not(Test-Path -LiteralPath $profile)){[IO.File]::WriteAllText($profile,'@{ Listen=''0.0.0.0:3240''; DiscoveryPort=3241; AllowedPeers=@(''127.0.0.1''); Export=@(); Debug=$false; ExpectedInstanceIds=@{} }',[Text.UTF8Encoding]::new($true))}
  if(-not(Test-Path -LiteralPath $config)){
   [ordered]@{version=1;enabled=$false;server_id=[guid]::NewGuid().ToString();name=$env:COMPUTERNAME;runtime=$runtime;profile=$profile;allowed_ips=@('127.0.0.1');probe_address='127.0.0.1:3240';discovery_port=3241;auto_remove_unplugged=$true;unplugged_retention_days=1}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $config -Encoding utf8
  }
  $state=Join-Path $runtime 'backend-state.json'
  if(-not(Test-Path -LiteralPath $state)){[ordered]@{Backend='Stopped';DesiredBackend='Native';NativePid=$null;NativeStartedUtc=$null;NativeCreatedFileTime=$null;ForcedByUs=@();Reservations=@();PendingStockReleases=@();ExportedBusIds=@()}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $state -Encoding utf8}
  & (Join-Path $runtime 'Set-Autostart.ps1') -Role Server -RuntimeDirectory $runtime -Config $config -Profile $profile
 }else{
  if(-not(Get-Service usbip2_ude -ErrorAction SilentlyContinue)){$driverResult=@(& (Join-Path $runtime 'Install-ProductionDrivers.ps1') -Role Client);if(-not $?){throw 'Client driver installation failed.'}}
  & (Join-Path $runtime 'Set-Autostart.ps1') -Role Client -RuntimeDirectory $runtime -Config $config
 }
 if(-not $?){throw 'Startup configuration failed.'}
 [ordered]@{schema=1;runtime=$runtime;initialized=$true}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $roleData 'installation.local.json') -Encoding utf8
 if(@($driverResult|Where-Object {$_.PSObject.Properties['RestartRequired']-and $_.RestartRequired}).Count){
  Add-Type -AssemblyName System.Windows.Forms
  [Windows.Forms.MessageBox]::Show('The signed USB driver requires a restart. Restart Windows manually before connecting USB devices.','USB to Network Bridge setup','OK','Information')|Out-Null
 }
 Start-Process -FilePath $agent -ArgumentList '--gui','--role',$Role.ToLowerInvariant(),'--config',('"'+$config+'"') -WindowStyle Hidden
}catch{
 Add-Type -AssemblyName System.Windows.Forms
 [Windows.Forms.MessageBox]::Show($_.Exception.Message,'USB to Network Bridge setup','OK','Error')|Out-Null
 exit 1
}
