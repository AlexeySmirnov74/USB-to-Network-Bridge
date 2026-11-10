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
$healthy=$true
foreach($entry in @(@('supervisor.local.json','--supervise'),@('health.local.json',$mode))){
 $path=[IO.Path]::ChangeExtension($Config,$entry[0])
 try {
  $beat=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json
  $age=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-[long]$beat.updated_unix_ms
  $process=Get-CimInstance Win32_Process -Filter ('ProcessId='+[uint32]$beat.process_id) -ErrorAction SilentlyContinue
  if($entry[0]-eq 'health.local.json'-and $process-and $process.CommandLine.Contains('--supervise')){$healthy=$false}
  if(-not $beat.running-or $beat.role-ne $Role.ToLowerInvariant()-or $age-lt -5000-or $age-gt 120000-or -not $process-or $process.ExecutablePath-ne $agent-or -not $process.CommandLine.Contains($entry[1])-or -not $process.CommandLine.Contains($Config)){$healthy=$false}
 }catch{$healthy=$false}
}
if($healthy){[ordered]@{action='already_healthy'}|ConvertTo-Json -Compress;exit 0}
$before=[string]$task.State
if($task.State-eq 'Running'){$task|Stop-ScheduledTask}
$task|Start-ScheduledTask
[ordered]@{action='restart_requested';previous_state=$before;task=$task.TaskName}|ConvertTo-Json -Compress
