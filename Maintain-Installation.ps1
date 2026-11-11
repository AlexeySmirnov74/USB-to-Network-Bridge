# Runs only from Windows Installer, elevated. Operates on this exact installation.
param([Parameter(Mandatory=$true)][ValidateSet('Stop','Resume','Remove')][string]$Operation,[Parameter(Mandatory=$true)][string]$RuntimeDirectory,[switch]$AfterInstall)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$runtime=[IO.Path]::GetFullPath($RuntimeDirectory).TrimEnd('\')
$agent=Join-Path $runtime 'usbbridge-agent.exe'
$camera=Join-Path $runtime 'usbbridge-camera.exe'
$ownedPrograms=@($agent,$camera)
if(-not (Test-Path -LiteralPath (Join-Path $runtime 'Maintain-Installation.ps1'))){exit 0}
if((Get-Item -LiteralPath $runtime).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Refusing a reparse-point installation.'}
$data=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge'
$journalDir=Join-Path $data 'Maintenance'
New-Item -ItemType Directory -Path $journalDir -Force|Out-Null
$acl=New-Object Security.AccessControl.DirectorySecurity;$acl.SetAccessRuleProtection($true,$false)
foreach($id in @('S-1-5-18','S-1-5-32-544')){$sid=New-Object Security.Principal.SecurityIdentifier($id);$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))}
Set-Acl -LiteralPath $journalDir -AclObject $acl
$hash=[Security.Cryptography.SHA256]::Create();$key=([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($runtime.ToLowerInvariant())))).Replace('-','').Substring(0,24);$hash.Dispose()
$journalPath=Join-Path $journalDir ($key+'.local.json')
function Save-Journal($Value){$tmp=$journalPath+'.tmp';$Value|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $tmp -Encoding utf8;Move-Item -LiteralPath $tmp -Destination $journalPath -Force}
if($Operation-eq 'Resume'){
 if(-not(Test-Path -LiteralPath $journalPath)){exit 0}
 $journal=Get-Content -LiteralPath $journalPath -Raw|ConvertFrom-Json
 if($journal.runtime-ne $runtime){throw 'Maintenance journal belongs to another installation.'}
 # A later fresh installation must not resurrect roles explicitly removed earlier.
 if($AfterInstall-and $journal.removal_requested){Remove-Item -LiteralPath $journalPath -Force;exit 0}
 foreach($marker in @($journal.markers|Where-Object {$_})){
  $path=Join-Path $data ($marker.role+'\installation.local.json')
  if($marker.role-notin @('Server','Client')){throw 'Invalid maintenance marker role.'}
  [IO.File]::WriteAllText($path,$marker.content,[Text.UTF8Encoding]::new($true))
 }
 foreach($entry in @($journal.tasks)){
  $task=Get-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue
  if(-not $task){Register-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -Xml $entry.xml|Out-Null}
  elseif(@($task.Actions).Count-ne 1-or @($task.Actions|Where-Object {$_.Execute-notin $ownedPrograms}).Count){throw 'A different installation now owns a startup task.'}
  $xml=[xml]$entry.xml
  $wasEnabled=([string]$xml.Task.Settings.Enabled)-ne 'false'
  if($wasEnabled){Enable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}else{Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}
  if($entry.running-and $wasEnabled){Start-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}
 }
 foreach($entry in @($journal.firewall)){
  if(-not(Get-NetFirewallRule -Name $entry.name -ErrorAction SilentlyContinue)){New-NetFirewallRule -Name $entry.name -DisplayName $entry.display -Direction Inbound -Action Allow -Program $entry.program -Protocol $entry.protocol -LocalPort $entry.port -RemoteAddress $entry.remote -Profile Any|Out-Null}
 }
 Remove-Item -LiteralPath $journalPath -Force
 exit 0
}
$tasks=@(Get-ScheduledTask -ErrorAction Stop|Where-Object {@($_.Actions|Where-Object {$_.Execute-in $ownedPrograms}).Count})
if($Operation-eq 'Stop'-and (Test-Path -LiteralPath $journalPath)){
 $previous=Get-Content -LiteralPath $journalPath -Raw|ConvertFrom-Json
 if($previous.runtime-ne $runtime){throw 'Maintenance journal belongs to another installation.'}
 if($previous.removal_requested){Remove-Item -LiteralPath $journalPath -Force}
}
if($Operation-eq 'Stop'-and -not(Test-Path -LiteralPath $journalPath)){
 $journal=[ordered]@{runtime=$runtime;tasks=@();firewall=@();markers=@();removal_requested=$false}
 foreach($task in $tasks){if(@($task.Actions).Count-ne 1){throw 'Unexpected multi-action startup task.'};$journal.tasks+=@([ordered]@{path=$task.TaskPath;name=$task.TaskName;running=($task.State-eq 'Running');xml=(Export-ScheduledTask -TaskPath $task.TaskPath -TaskName $task.TaskName)})}
 foreach($rule in @(Get-NetFirewallRule -Name 'USBBridge-*' -ErrorAction SilentlyContinue)){
  $app=$rule|Get-NetFirewallApplicationFilter
  if($app.Program-in @($agent,$camera,(Join-Path $runtime 'usbbridge-server.exe'))){$port=$rule|Get-NetFirewallPortFilter;$address=$rule|Get-NetFirewallAddressFilter;$journal.firewall+=@([ordered]@{name=$rule.Name;display=$rule.DisplayName;program=$app.Program;protocol=$port.Protocol;port=$port.LocalPort;remote=$address.RemoteAddress})}
 }
 foreach($role in @('Server','Client')){
  if(@($tasks|Where-Object TaskName -eq ('USBBridge.'+$role)).Count){
   $marker=Join-Path $data ($role+'\installation.local.json')
   if(Test-Path -LiteralPath $marker){$journal.markers+=@([ordered]@{role=$role;content=[IO.File]::ReadAllText($marker)})}
  }
 }
 Save-Journal $journal
}
foreach($task in $tasks){$task|Disable-ScheduledTask|Out-Null}
foreach($task in @($tasks|Where-Object {$_.Actions.Execute-eq $camera-and $_.Actions.Arguments-match '^receive --config "(.+)"$'})){
 $videoConfig=[regex]::Match($task.Actions.Arguments,'^receive --config "(.+)"$').Groups[1].Value
 if(Test-Path -LiteralPath $videoConfig){$video=Get-Content -LiteralPath $videoConfig -Raw -Encoding UTF8|ConvertFrom-Json;& $camera detach-local ('127.0.0.1:'+$video.usb_port)|Out-Null;if($LASTEXITCODE-ne 0){throw "Could not release this installation's virtual USB camera."}}
}
foreach($task in $tasks){$task|Stop-ScheduledTask -ErrorAction SilentlyContinue}
foreach($item in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe' OR Name='usbbridge-camera.exe'"|Where-Object {$_.ExecutablePath-in $ownedPrograms})){
 $p=Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue;if($p){Stop-Process -Id $p.Id -Force;$p.WaitForExit()}
}
# Pause this installation's remembered imports while preserving its original choices.
$clientConfig=Join-Path $data 'Client\client.local.json'
if(@($tasks|Where-Object TaskName -eq 'USBBridge.Client').Count-and (Test-Path -LiteralPath $clientConfig)){
 $original=[IO.File]::ReadAllBytes($clientConfig)
 try{
  $client=Get-Content -LiteralPath $clientConfig -Raw|ConvertFrom-Json
  $servers=@($client)+@($client.servers)
  foreach($entry in $servers){if($entry.server){$selector=if($entry.server_id){$entry.server_id}else{$entry.server};& (Join-Path $runtime 'usbbridge.exe') --config $clientConfig client pause $selector|Out-Null;if($LASTEXITCODE-ne 0){throw 'Could not detach an import owned by this installation.'}}}
 }finally{[IO.File]::WriteAllBytes($clientConfig,$original)}
}
$serverConfig=Join-Path $data 'Server\host.local.json'
if(@($tasks|Where-Object TaskName -eq 'USBBridge.Server').Count-and (Test-Path -LiteralPath $serverConfig)){
 $hostData=Get-Content -LiteralPath $serverConfig -Raw|ConvertFrom-Json
 if($hostData.runtime-ne $runtime){throw 'Server configuration belongs to another installation.'}
 $state=Get-Content -LiteralPath (Join-Path $runtime 'backend-state.json') -Raw|ConvertFrom-Json
 & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $state.DesiredBackend -RuntimeDirectory $runtime -Profile $hostData.profile -Stop
 if(-not $?){throw 'Could not stop the owned USB transport.'}
}
if($Operation-eq 'Remove'){
 if(Test-Path -LiteralPath $journalPath){
  $journal=Get-Content -LiteralPath $journalPath -Raw|ConvertFrom-Json
  if($journal.runtime-ne $runtime){throw 'Maintenance journal belongs to another installation.'}
  $journal|Add-Member -NotePropertyName removal_requested -NotePropertyValue $true -Force
  Save-Journal $journal
 }
 foreach($task in $tasks){
  $task|Unregister-ScheduledTask -Confirm:$false
  if($task.TaskName-in @('USBBridge.Server','USBBridge.Client')){
   $role=$task.TaskName.Split('.')[-1]
   $marker=Join-Path $data ($role+'\installation.local.json')
   if(Test-Path -LiteralPath $marker){[ordered]@{schema=1;runtime=$runtime;initialized=$false}|ConvertTo-Json|Set-Content -LiteralPath $marker -Encoding utf8}
  }
 }
 foreach($rule in @(Get-NetFirewallRule -Name 'USBBridge-*' -ErrorAction SilentlyContinue)){
  if(($rule|Get-NetFirewallApplicationFilter).Program-in @($agent,$camera,(Join-Path $runtime 'usbbridge-server.exe'))){$rule|Remove-NetFirewallRule}
 }
}
