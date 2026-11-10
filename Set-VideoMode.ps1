# Configure only this runtime's video endpoint. Physical USB sessions are untouched.
param([Parameter(Mandatory=$true)][ValidateSet('Receive','Send')][string]$Role,[Parameter(Mandatory=$true)][string]$Config,[Parameter(Mandatory=$true)][string]$RequestFile,[string]$InteractiveUser,[switch]$Elevate)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$Config=[IO.Path]::GetFullPath($Config);$RequestFile=[IO.Path]::GetFullPath($RequestFile)
$resultPath=[IO.Path]::ChangeExtension($Config,'operation.local.json')
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'Video task configuration requires Administrator rights.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Role',$Role,'-Config',('"'+$Config+'"'),'-RequestFile',('"'+$RequestFile+'"'))
 if($InteractiveUser){$arguments+=@('-InteractiveUser',('"'+$InteractiveUser+'"'))}
 $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $child.WaitForExit()
 if(Test-Path -LiteralPath $resultPath){Get-Content -LiteralPath $resultPath -Raw}
 exit $child.ExitCode
}
$runtime=$PSScriptRoot;$exe=Join-Path $runtime 'usbbridge-camera.exe';$mode=$Role.ToLowerInvariant();$taskName='USBBridge.Video.'+$Role;$taskPath='\USBBridge\'
$hash=[Security.Cryptography.SHA256]::Create();try{$suffix=([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($Config.ToLowerInvariant())))).Replace('-','').Substring(0,16)}finally{$hash.Dispose()}
$ruleName='USBBridge-Video-'+$suffix
$oldTask=$null;$oldXml=$null;$oldBytes=$null;$oldConfig=$null;$changed=$false;$oldRunning=$false;$oldRule=$null;$operationLock=$null;$healthTask=$null;$oldHealthXml=$null;$healthName='USBBridge.Video.Health.'+$Role
function Stop-OwnedVideo {
 foreach($item in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-camera.exe'")){
  $selected='(?:^|\s)--config\s+(?:"'+[regex]::Escape($Config)+'"|'+[regex]::Escape($Config)+'(?:\s|$))'
  if($item.ExecutablePath-eq $exe-and $item.CommandLine-match $selected-and $item.CommandLine-match ('(?:^|\s)'+$mode+'(?:\s|$)')){
   $process=Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue
   if($process){$null=$process.Handle;Stop-Process -InputObject $process -Force -ErrorAction SilentlyContinue;if(-not $process.WaitForExit(10000)){throw 'Owned video process did not exit.'}}
  }
 }
}
try {
 $operationLock=[IO.File]::Open([IO.Path]::ChangeExtension($Config,'operation-video.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
 if(-not(Test-Path -LiteralPath $exe -PathType Leaf)){throw 'Install the complete application package before configuring video.'}
 $request=Get-Content -LiteralPath $RequestFile -Raw -Encoding UTF8|ConvertFrom-Json
 & $exe validate $mode $RequestFile|Out-Null
 if($LASTEXITCODE-ne 0){throw 'Video settings validation failed.'}
 if($Role-eq 'Receive'-and $request.enabled){& $exe driver-ready|Out-Null;if($LASTEXITCODE-ne 0){throw 'Install the signed USBIP-WIN2 client driver on this receiver; its installation may require a manual Windows restart.'}}
 if($Role-eq 'Send'-and $request.enabled-and -not $InteractiveUser){throw 'An interactive Windows user is required for OBS/Unity video capture.'}
 $oldTask=Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
 $expected=$mode+' --config "'+$Config+'"'
 if($oldTask){if(@($oldTask.Actions).Count-ne 1-or $oldTask.Actions[0].Execute-ne $exe-or $oldTask.Actions[0].Arguments-ne $expected){throw 'The video task belongs to another runtime/configuration.'};$oldXml=Export-ScheduledTask -TaskPath $taskPath -TaskName $taskName;$oldRunning=$oldTask.State-eq 'Running'}
 $healthTask=Get-ScheduledTask -TaskPath $taskPath -TaskName $healthName -ErrorAction SilentlyContinue
 if($healthTask){if(@($healthTask.Actions).Count-ne 1-or $healthTask.Actions[0].Execute-ne $exe-or $healthTask.Actions[0].Arguments-ne ('check-'+$mode+' --config "'+$Config+'"')){throw 'The video health task belongs to another runtime/configuration.'};$oldHealthXml=Export-ScheduledTask -TaskPath $taskPath -TaskName $healthName}
 if(Test-Path -LiteralPath $Config){$oldBytes=[IO.File]::ReadAllBytes($Config);$oldConfig=Get-Content -LiteralPath $Config -Raw -Encoding UTF8|ConvertFrom-Json}
 $rule=Get-NetFirewallRule -Name $ruleName -ErrorAction SilentlyContinue
 if($rule){$app=$rule|Get-NetFirewallApplicationFilter;if($app.Program-ne $exe){throw 'The video firewall rule belongs to another application.'};$port=$rule|Get-NetFirewallPortFilter;$address=$rule|Get-NetFirewallAddressFilter;$oldRule=[ordered]@{port=$port.LocalPort;remote=$address.RemoteAddress}}
 if($healthTask){$healthTask|Disable-ScheduledTask|Out-Null;$healthTask|Stop-ScheduledTask -ErrorAction SilentlyContinue}
 if($oldTask){$oldTask|Disable-ScheduledTask|Out-Null;$oldTask|Stop-ScheduledTask -ErrorAction SilentlyContinue}
 if($Role-eq 'Receive'-and $oldConfig){& $exe detach-local ('127.0.0.1:'+$oldConfig.usb_port)|Out-Null;if($LASTEXITCODE-ne 0){throw 'Could not release the owned virtual USB camera.'}}
 Stop-OwnedVideo
 $changed=$true
 [IO.File]::WriteAllText($Config,($request|ConvertTo-Json -Depth 10),[Text.UTF8Encoding]::new($false))
 if($rule){$rule|Remove-NetFirewallRule}
 if($request.enabled){
  if($Role-eq 'Receive'){
   $principal=New-ScheduledTaskPrincipal -UserId SYSTEM -LogonType ServiceAccount -RunLevel Highest
   $startup=New-ScheduledTaskTrigger -AtStartup
   New-NetFirewallRule -Name $ruleName -DisplayName 'USB to Network Bridge: incoming video' -Direction Inbound -Action Allow -Program $exe -Protocol TCP -LocalPort $request.video_port -RemoteAddress @($request.allowed_ips) -Profile Any|Out-Null
  }else{$principal=New-ScheduledTaskPrincipal -UserId $InteractiveUser -LogonType Interactive -RunLevel Limited;$startup=New-ScheduledTaskTrigger -AtLogOn -User $InteractiveUser}
  $action=New-ScheduledTaskAction -Execute $exe -Argument $expected -WorkingDirectory $runtime
  $settings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -RestartCount 255 -RestartInterval (New-TimeSpan -Minutes 1)
  $retry=New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
  Register-ScheduledTask -TaskPath $taskPath -TaskName $taskName -Action $action -Principal $principal -Trigger @($startup,$retry) -Settings $settings -Description 'Persistent USB camera video endpoint; restart when unavailable.' -Force|Out-Null
  $healthAction=New-ScheduledTaskAction -Execute $exe -Argument ('check-'+$mode+' --config "'+$Config+'"') -WorkingDirectory $runtime
  $healthPrincipal=New-ScheduledTaskPrincipal -UserId SYSTEM -LogonType ServiceAccount -RunLevel Highest
  $healthSettings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskPath $taskPath -TaskName $healthName -Action $healthAction -Principal $healthPrincipal -Trigger $retry -Settings $healthSettings -Description 'Check video endpoint heartbeat every minute; recover after 120 seconds without progress.' -Force|Out-Null
  $startedAt=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
  Start-ScheduledTask -TaskPath $taskPath -TaskName $taskName
  $statusPath=[IO.Path]::ChangeExtension($Config,'status.json');$deadline=[DateTime]::UtcNow.AddSeconds(12);$ready=$false
  do {
   Start-Sleep -Milliseconds 400
   try{$status=Get-Content -LiteralPath $statusPath -Raw -Encoding UTF8|ConvertFrom-Json;$age=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-[long]$status.updated_unix_ms;$ready=[long]$status.updated_unix_ms-ge $startedAt-and $age-ge -5000-and $age-lt 5000-and $status.running-and ($Role-ne 'Receive'-or ($status.device_attached-and $status.name_applied))}catch{}
  }while(-not $ready-and [DateTime]::UtcNow-lt $deadline)
  if(-not $ready){throw 'Video endpoint did not become ready; inspect its component log.'}
 }
 [ordered]@{success=$true;role=$Role;enabled=[bool]$request.enabled;config=$Config;restart_issued=$false}|ConvertTo-Json|Set-Content -LiteralPath $resultPath -Encoding utf8
}catch{
 $failure=$_.Exception.Message
 try {
  if($changed){$current=Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue;if($current){$current|Disable-ScheduledTask|Out-Null;$current|Stop-ScheduledTask};Stop-OwnedVideo;if($oldBytes){[IO.File]::WriteAllBytes($Config,$oldBytes)}elseif(Test-Path -LiteralPath $Config){Remove-Item -LiteralPath $Config -Force}}
  if($oldHealthXml){Register-ScheduledTask -TaskPath $taskPath -TaskName $healthName -Xml $oldHealthXml -Force|Out-Null}elseif($changed){Unregister-ScheduledTask -TaskPath $taskPath -TaskName $healthName -Confirm:$false -ErrorAction SilentlyContinue}
  if($oldXml){Register-ScheduledTask -TaskPath $taskPath -TaskName $taskName -Xml $oldXml -Force|Out-Null;if($oldRunning){Start-ScheduledTask -TaskPath $taskPath -TaskName $taskName}}
  elseif($changed){Unregister-ScheduledTask -TaskPath $taskPath -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue}
  if($changed){$current=Get-NetFirewallRule -Name $ruleName -ErrorAction SilentlyContinue;if($current-and ($current|Get-NetFirewallApplicationFilter).Program-eq $exe){$current|Remove-NetFirewallRule};if($oldRule){New-NetFirewallRule -Name $ruleName -DisplayName 'USB to Network Bridge: incoming video' -Direction Inbound -Action Allow -Program $exe -Protocol TCP -LocalPort $oldRule.port -RemoteAddress $oldRule.remote -Profile Any|Out-Null}}
 }catch{$failure+='; rollback: '+$_.Exception.Message}
 [ordered]@{success=$false;role=$Role;error=$failure}|ConvertTo-Json|Set-Content -LiteralPath $resultPath -Encoding utf8
 exit 1
}finally{if($operationLock){$operationLock.Dispose()}}
