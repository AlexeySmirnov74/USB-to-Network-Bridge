# Independent video watchdog. Recover only the registered, enabled endpoint.
param([Parameter(Mandatory=$true)][ValidateSet('Receive','Send','LocalSend')][string]$Role,[Parameter(Mandatory=$true)][string]$Config)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$Config=[IO.Path]::GetFullPath($Config);$exe=Join-Path $PSScriptRoot 'usbbridge-camera.exe';$mode=if($Role-eq 'Receive'){'receive'}else{'send'};$healthMode=if($Role-eq 'LocalSend'){'local-send'}else{$mode};$taskName='USBBridge.Video.'+$Role;$taskPath='\USBBridge\';$lease=$null
try {
 try{$lease=[IO.File]::Open([IO.Path]::ChangeExtension($Config,'operation-video.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)}catch [IO.IOException]{[ordered]@{action='configuration_busy'}|ConvertTo-Json -Compress;exit 0}
 $task=Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue
 if(-not $task-or $task.State-eq 'Disabled'){[ordered]@{action='disabled_or_not_registered'}|ConvertTo-Json -Compress;exit 0}
 $expected=$mode+' --config "'+$Config+'"'
 if(@($task.Actions).Count-ne 1-or $task.Actions[0].Execute-ne $exe-or $task.Actions[0].Arguments-ne $expected){throw 'Video health refused a task owned by another runtime/configuration.'}
 $configData=Get-Content -LiteralPath $Config -Raw -Encoding UTF8|ConvertFrom-Json
 if(-not $configData.enabled){[ordered]@{action='disabled_in_settings'}|ConvertTo-Json -Compress;exit 0}
 $selector='(?:^|\s)--config\s+(?:"'+[regex]::Escape($Config)+'"|'+[regex]::Escape($Config)+'(?:\s|$))'
 function Processes {@(Get-CimInstance Win32_Process -Filter "Name='usbbridge-camera.exe'"|Where-Object {$_.ExecutablePath-eq $exe-and $_.CommandLine-match $selector-and $_.CommandLine-match ('(?:^|\s)'+$mode+'(?:\s|$)')})}
 function Fresh {
  try {$status=Get-Content -LiteralPath ([IO.Path]::ChangeExtension($Config,'status.json')) -Raw -Encoding UTF8|ConvertFrom-Json;$age=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()-[long]$status.updated_unix_ms
   return $status.running-and $age-ge -5000-and $age-le 120000-and @((Processes)|Where-Object ProcessId -eq $status.process_id).Count-eq 1
  }catch{return $false}
 }
 if(Fresh){[ordered]@{action='healthy'}|ConvertTo-Json -Compress;exit 0}
 $processes=@(Processes)
 if(@($processes|Where-Object {([DateTime]::UtcNow-$_.CreationDate.ToUniversalTime()).TotalSeconds-lt 120}).Count){[ordered]@{action='startup_grace'}|ConvertTo-Json -Compress;exit 0}
 # Allow an ordinary sleep/resume gap to advance the heartbeat before recovery.
 if($processes.Count){Start-Sleep -Seconds 3;if(Fresh){[ordered]@{action='resumed'}|ConvertTo-Json -Compress;exit 0}}
 $handles=@($processes|ForEach-Object {$p=Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue;if($p){$null=$p.Handle;$p}})
 if($task.State-eq 'Running'){$task|Stop-ScheduledTask}
 foreach($p in $handles){Stop-Process -InputObject $p -Force -ErrorAction SilentlyContinue;if(-not $p.WaitForExit(10000)){throw 'The owned video endpoint has not exited.'}}
 $deadline=[DateTime]::UtcNow.AddSeconds(10)
 do {$task=Get-ScheduledTask -TaskPath $taskPath -TaskName $taskName;if($task.State-eq 'Disabled'){[ordered]@{action='disabled_during_recovery'}|ConvertTo-Json -Compress;exit 0};if($task.State-ne 'Running'){break};Start-Sleep -Milliseconds 100}while([DateTime]::UtcNow-lt $deadline)
 if($task.State-eq 'Running'){throw 'The video task is still stopping.'}
 $task|Start-ScheduledTask
 [ordered]@{action='restart_requested';role=$Role;previous_pids=@($processes|Select-Object -ExpandProperty ProcessId)}|ConvertTo-Json -Compress
}finally{if($lease){$lease.Dispose()}}
