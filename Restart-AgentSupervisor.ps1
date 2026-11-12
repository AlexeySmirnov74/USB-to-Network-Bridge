# Called by the independent scheduled health check; operate only on the registered role.
param([Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,[Parameter(Mandatory=$true)][string]$Config)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$Config=[IO.Path]::GetFullPath($Config)
$agent=Join-Path $PSScriptRoot 'usbbridge-agent.exe'
$mode=if($Role-eq 'Server'){'--host'}else{'--client'}
$expected='--supervise '+$mode+' --config "'+$Config+'"'
$task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName ('USBBridge.'+$Role) -ErrorAction SilentlyContinue
if(-not $task-or $task.State-eq 'Disabled'){[ordered]@{action='disabled_or_not_registered'}|ConvertTo-Json -Compress;exit 0}
if(@($task.Actions).Count-ne 1-or $task.Actions[0].Execute-ne $agent-or $task.Actions[0].Arguments-ne $expected){throw 'Health check refused to control a task owned by another runtime/configuration.'}
$healthy=$true;$controller=$null;$supervisorPid=$null;$supervisorProcess=$null
foreach($entry in @(@('supervisor.local.json','--supervise'),@('health.local.json',$mode))){
 $path=[IO.Path]::ChangeExtension($Config,$entry[0])
 try {
  $beat=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json
  $age=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-[long]$beat.updated_unix_ms
  $process=Get-CimInstance Win32_Process -Filter ('ProcessId='+[uint32]$beat.process_id) -ErrorAction SilentlyContinue
  if($entry[0]-eq 'supervisor.local.json'){$supervisorPid=[uint32]$beat.process_id;$supervisorProcess=$process}
  if($entry[0]-eq 'health.local.json'){$controller=$process}
  if($entry[0]-eq 'health.local.json'-and $process-and $process.CommandLine-match '(?:^|\s)--supervise(?:\s|$)'){$healthy=$false}
  if(-not $beat.running-or $beat.role-ne $Role.ToLowerInvariant()-or $age-lt -5000-or $age-gt 120000-or -not $process-or $process.ExecutablePath-ne $agent-or -not $process.CommandLine.Contains($entry[1])-or -not $process.CommandLine.Contains($Config)){$healthy=$false}
 }catch{$healthy=$false}
}
if($healthy){[ordered]@{action='already_healthy'}|ConvertTo-Json -Compress;exit 0}
# A frozen child cannot observe its parent's exit. Clean up only a controller
# explicitly launched by this supervisor for this exact role/configuration.
if($controller-and $supervisorPid-and $controller.ExecutablePath-eq $agent-and $controller.CommandLine.Contains($Config)-and
   $controller.CommandLine-match ('(?:^|\s)--supervised-by\s+'+$supervisorPid+'(?:\s|$)')-and
   $controller.CommandLine-match ('(?:^|\s)'+[regex]::Escape($mode)+'(?:\s|$)')-and
   $controller.CommandLine-notmatch '(?:^|\s)--supervise(?:\s|$)'){
 $owned=Get-Process -Id $controller.ProcessId -ErrorAction SilentlyContinue
 if($owned){Stop-Process -InputObject $owned -Force -ErrorAction SilentlyContinue}
}
$before=[string]$task.State
# Task Scheduler stop is asynchronous. Retain a process handle, terminate only
# the validated supervisor, and wait for it to release its singleton mutex.
$ownedSupervisor=$null
if($supervisorProcess-and $supervisorProcess.ExecutablePath-eq $agent-and
   $supervisorProcess.CommandLine.Contains($Config)-and
   $supervisorProcess.CommandLine-match '(?:^|\s)--supervise(?:\s|$)'-and
   $supervisorProcess.CommandLine-match ('(?:^|\s)'+[regex]::Escape($mode)+'(?:\s|$)')){
 $ownedSupervisor=Get-Process -Id $supervisorProcess.ProcessId -ErrorAction SilentlyContinue
 if($ownedSupervisor){$null=$ownedSupervisor.Handle}
}
if($task.State-eq 'Running'){$task|Stop-ScheduledTask}
if($ownedSupervisor){
 Stop-Process -InputObject $ownedSupervisor -Force -ErrorAction SilentlyContinue
 if(-not $ownedSupervisor.WaitForExit(10000)){throw 'The owned supervisor has not exited; retry recovery on the next health check.'}
}
$deadline=[DateTime]::UtcNow.AddSeconds(10)
do {
 $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName ('USBBridge.'+$Role) -ErrorAction Stop
 if($task.State-eq 'Disabled'){[ordered]@{action='disabled_during_recovery'}|ConvertTo-Json -Compress;exit 0}
 if($task.State-ne 'Running'){break}
 Start-Sleep -Milliseconds 100
}while([DateTime]::UtcNow-lt $deadline)
if($task.State-eq 'Running'){throw 'The supervisor task is still stopping; retry recovery on the next health check.'}
$application=& (Join-Path $PSScriptRoot 'usbbridge.exe') --config $Config ($Role.ToLowerInvariant()) application-state status|Out-String
if($LASTEXITCODE-ne 0){throw 'Cannot confirm whether this application was manually stopped.'}
$application=$application|ConvertFrom-Json
if($application.stopped-isnot [bool]){throw 'The application stop state is invalid.'}
if($application.stopped){[ordered]@{action='manually_stopped'}|ConvertTo-Json -Compress;exit 0}
$task|Start-ScheduledTask
[ordered]@{action='restart_requested';previous_state=$before;task=$task.TaskName}|ConvertTo-Json -Compress
