# First-run setup for MSI and portable copies; signed drivers stay upstream.
param(
 [Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,
 [switch]$Elevate,[string]$Config,[string]$InstallRoot,
 [ValidateSet('New','Keep')][string]$ClientChoice='New',[switch]$AdoptExisting
)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$runtime=$PSScriptRoot
if(-not $InstallRoot){$InstallRoot=if((Split-Path -Leaf $runtime)-eq 'app'){Split-Path -Parent $runtime}else{$runtime}}
$InstallRoot=[IO.Path]::GetFullPath($InstallRoot)
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'First-run setup requires Administrator rights.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Role',$Role,'-InstallRoot',('"'+$InstallRoot+'"'),'-ClientChoice',$ClientChoice)
 if($Config){$arguments+=@('-Config',('"'+[IO.Path]::GetFullPath($Config)+'"'))}
 if($AdoptExisting){$arguments+='-AdoptExisting'}
 $p=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $p.WaitForExit();exit $p.ExitCode
}
try {
 . (Join-Path $runtime 'FirstRun-Identity.ps1')
 . (Join-Path $runtime 'Update-PortableSettings.ps1') -FunctionsOnly -Role $Role -Config $Config -InstallRoot $InstallRoot
 $agent=Join-Path $runtime 'usbbridge-agent.exe'
 if(-not(Test-Path -LiteralPath $agent -PathType Leaf)){throw 'The complete app folder is required for setup.'}
 $property=$Role+'Directory'
 $registration=Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -Name $property -ErrorAction SilentlyContinue
 $registered=if($registration){[string]$registration.$property}else{''}
 $installed=$registered-and ((Test-BridgeSamePath $registered $InstallRoot)-or (Test-BridgeSamePath $registered $runtime))
 $common=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge'
 $roleData=if($installed-or $Role-eq 'Server'){Join-Path $common $Role}else{Join-Path $runtime 'data\Client'}
 $filename=if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'}
 $sourceConfig=if($Config){[IO.Path]::GetFullPath($Config)}else{Join-Path $roleData $filename}
 $machine=Get-BridgeMachineIdentity
 $owned=(Test-BridgeOwnedRoleTask $Role $runtime $sourceConfig)-or (Test-BridgeOwnedRoleProcess $Role $runtime $sourceConfig)
 $disposition=Get-BridgeMachineDisposition -Role $Role -Runtime $runtime -Config $sourceConfig -MachineIdentity $machine -OwnedTask:$owned
 $legacy=if($installed){Read-BridgeJson (Join-Path $roleData 'installation.local.json')}else{$null}
 $legacyOwned=Test-BridgeInstalledRoleMarker -Marker $legacy -Role $Role -Runtime $runtime -InstallRoot $InstallRoot -Config $sourceConfig -MarkerPath (Join-Path $roleData 'installation.local.json')
 $preserve=$AdoptExisting-and ($disposition-in @('Current','Adopt')-or $legacyOwned)
 if($AdoptExisting-and -not $preserve){throw 'The existing configuration is not verified for this computer; reopen the launcher to choose first-run settings.'}
 $config=if($preserve){$sourceConfig}else{Join-Path $roleData $filename}
 # Ownership is checked before touching code, role data or driver installation.
 $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName ('USBBridge.'+$Role) -ErrorAction SilentlyContinue
 if($task-and @($task.Actions|Where-Object {-not(Test-BridgeSamePath $_.Execute $agent)}).Count){throw 'Another installation owns this role. Migrate or disable its startup before configuring this installation.'}
 if($task-and -not(Test-BridgeOwnedRoleTask $Role $runtime $config)){throw 'Another configuration owns this role startup; it was not changed.'}

 function Protect-Directory([string]$Path,[bool]$UserWrite){
  New-Item -ItemType Directory -Path $Path -Force|Out-Null
  if((Get-Item -LiteralPath $Path).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'An application/configuration directory must not be a junction or symlink.'}
  $acl=New-Object Security.AccessControl.DirectorySecurity
  $acl.SetAccessRuleProtection($true,$false)
  $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
  foreach($entry in @(@('S-1-5-18','FullControl'),@('S-1-5-32-544','FullControl'),@('S-1-5-32-545',$(if($UserWrite){'Modify'}else{'ReadAndExecute'})))){
   $sid=New-Object Security.Principal.SecurityIdentifier($entry[0])
   $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,$entry[1],'ContainerInherit,ObjectInherit','None','Allow')))
  }
  Set-Acl -LiteralPath $Path -AclObject $acl
 }
 function Assert-SafeAncestors([string]$Path){
  $node=Get-Item -LiteralPath $Path
  while($node){
   if($node.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'The application path must not contain a junction or symlink.'}
   if(Test-BridgeBroadReplacementPermission (Get-Acl -LiteralPath $node.FullName).Access){throw ('This parent folder permits unprivileged code replacement: '+$node.FullName+'. Extract the portable folder into a protected location (for example Program Files), then reopen it. Setup did not change this parent folder.')}
   $node=$node.Parent
  }
 }
 function Allow-UserRequests([string]$Path){
  # Users can create their own request files, never overwrite administrator-
  # owned host/profile files. Credentials get their explicit private-file ACL.
  $acl=Get-Acl -LiteralPath $Path
  $users=New-Object Security.Principal.SecurityIdentifier('S-1-5-32-545')
  $creator=New-Object Security.Principal.SecurityIdentifier('S-1-3-0')
  $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($users,'CreateFiles','None','None','Allow')))
  $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($creator,'Modify','ObjectInherit','InheritOnly','Allow')))
  Set-Acl -LiteralPath $Path -AclObject $acl
 }
 # Existing, verified working tasks are never re-registered for a driver repair.
 # New SYSTEM tasks require protected code and protected parent directories.
 if(-not($preserve-and $task)){
  $ancestor=(Get-Item -LiteralPath $InstallRoot).Parent
  if($ancestor){Assert-SafeAncestors $ancestor.FullName}
  Protect-Directory $InstallRoot $false
  if(-not(Test-BridgeSamePath $runtime $InstallRoot)){Protect-Directory $runtime $false}
  # Reset inherited/broad file permissions too; leave user data to its own ACL.
  foreach($item in @(Get-ChildItem -LiteralPath $runtime -Recurse -Force)){
   if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'The app folder contains a junction or symlink.'}
   if($item.FullName.StartsWith((Join-Path $runtime 'data')+'\',[StringComparison]::OrdinalIgnoreCase)-or $item.FullName-match '\\first-run-backups(?:\\|$)'){continue}
   if($item.PSIsContainer){Protect-Directory $item.FullName $false}else{
    if($item.Extension-notin @('.exe','.msi','.dll','.ps1','.psm1','.psd1','.bat','.cmd')-and $item.Name-ne 'driver-installers.json'){continue}
    $acl=New-Object Security.AccessControl.FileSecurity
    $acl.SetAccessRuleProtection($true,$false)
    $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
    foreach($sidValue in @('S-1-5-18','S-1-5-32-544','S-1-5-32-545')){
     $rights=if($sidValue-eq 'S-1-5-32-545'){'ReadAndExecute'}else{'FullControl'}
     $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sidValue)),$rights,'Allow')))
    }
    Set-Acl -LiteralPath $item.FullName -AclObject $acl
   }
  }
 }
 if(Test-BridgeSamePath $roleData (Join-Path $common $Role)){Protect-Directory $common $false}
 Protect-Directory $roleData ($Role-eq 'Client')
 if($Role-eq 'Server'){
  $credentialDirectory=Join-Path $roleData 'video-credentials'
  Protect-Directory $credentialDirectory $false
 }

 $driverResult=& (Join-Path $runtime 'Ensure-ProductionDrivers.ps1') -Role $Role -RuntimeDirectory $runtime
 if(-not $?){throw 'Bundled signed driver setup failed.'}
 if(-not $preserve){
  $sourceExists=Test-Path -LiteralPath $sourceConfig -PathType Leaf
  if($sourceExists-and -not(Test-BridgeSamePath $sourceConfig $config)){
   $retained=if($Role-eq 'Client'-and $ClientChoice-eq 'Keep'){ConvertTo-BridgePortableClientSettings (Read-BridgeJson $sourceConfig)}else{$null}
   $preferences=if($retained){Read-BridgeJson ([IO.Path]::ChangeExtension($sourceConfig,'preferences.json'))}else{$null}
   $null=Backup-BridgeRoleState $Role $runtime $sourceConfig
   $null=Reset-BridgeRoleState -Role $Role -Runtime $runtime -Config $config
   if($retained){Write-BridgeJson $config $retained}
   if($preferences){Write-BridgeJson ([IO.Path]::ChangeExtension($config,'preferences.json')) $preferences}
  }else{
   $null=Reset-BridgeRoleState -Role $Role -Runtime $runtime -Config $config -KeepClientConnections:($Role-eq 'Client'-and $ClientChoice-eq 'Keep')
  }
 }
 if($Role-eq 'Server'){
  $hostConfig=Resolve-BridgeConfigPaths (Read-BridgeJson $config) $config $runtime
  $profile=if($hostConfig.profile-and (Test-Path -LiteralPath $hostConfig.profile)){[string]$hostConfig.profile}else{Join-Path (Split-Path -Parent $config) 'server.local.psd1'}
  if(-not(Test-Path -LiteralPath $profile)){throw 'The existing server profile is missing; restore it before startup.'}
  if(-not(Test-BridgeSamePath $hostConfig.runtime $runtime)){$hostConfig.runtime=$runtime;$hostConfig.profile=$profile;Write-BridgeJson $config (ConvertTo-BridgePortableConfig $hostConfig $config $runtime)}
 }
 if(-not($preserve-and $task)){
  $autostart=@{Role=$Role;RuntimeDirectory=$runtime;Config=$config}
  if($Role-eq 'Server'){$autostart.Profile=$profile}
  & (Join-Path $runtime 'Set-Autostart.ps1') @autostart
  if(-not $?){throw 'Startup configuration failed.'}
 }
 if($Role-eq 'Server'){
  # Grant request creation only after administrator-owned fixed config names
  # have been created, so first setup cannot inherit a user-created host file.
  Allow-UserRequests $roleData
  Allow-UserRequests $credentialDirectory
 }
 $null=Convert-BridgeRoleSettings -Role $Role -Runtime $runtime -Config $config -InstallRoot $InstallRoot
 $null=Write-BridgeMachineMarker -Role $Role -Runtime $runtime -Config $config -InstallRoot $InstallRoot -AdoptExisting:$preserve
 $null=Register-BridgeDeploymentBinding -Runtime $runtime -InstallRoot $InstallRoot -RoleConfigs @{$Role=$config}
 if($installed){
  # The copied role marker is portable too; OS registration keeps its locator.
  $portableMarker=Read-BridgeJson (Get-BridgeMachineMarkerPath $Role $runtime)
  Write-BridgeJson (Join-Path $roleData 'installation.local.json') $portableMarker
 }
 if($driverResult.RestartRequired){
  Add-Type -AssemblyName System.Windows.Forms
  [Windows.Forms.MessageBox]::Show('The signed USB driver is not ready yet or requested a restart. Restart Windows manually before connecting USB devices. Windows will not restart automatically.','USB to Network Bridge setup','OK','Information')|Out-Null
 }
 $stateText=& (Join-Path $runtime 'usbbridge.exe') --config $config $Role.ToLowerInvariant() application-state status
 if($LASTEXITCODE-ne 0){throw 'Could not inspect the application exit state.'}
 $state=($stateText|Out-String)|ConvertFrom-Json
 if($state.stopped){
  & (Join-Path $runtime 'Quit-Application.ps1') -Role $Role -Config $config -RuntimeDirectory $runtime -Resume
  if(-not $?){throw 'The configured application could not resume.'}
 }
 Start-Process -FilePath $agent -ArgumentList '--gui','--role',$Role.ToLowerInvariant(),'--config',('"'+$config+'"') -WorkingDirectory $runtime -WindowStyle Hidden
 exit 0
}catch{
 Add-Type -AssemblyName System.Windows.Forms
 [Windows.Forms.MessageBox]::Show($_.Exception.Message,'USB to Network Bridge setup','OK','Error')|Out-Null
 exit 1
}
