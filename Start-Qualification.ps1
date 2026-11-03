# Continuous observation/camera qualification. Never records video images or USB payloads.
param(
 [Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,
 [Parameter(Mandatory=$true)][string]$RuntimeDirectory,
 [Parameter(Mandatory=$true)][string]$Config,
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [ValidateRange(0.001,168)][double]$Hours=48,
 [string]$CameraIdentity,
 [string]$DiagnosticExecutable,
 [ValidateRange(0,240)][double]$MinimumFps=27,
 [ValidateRange(1,60)][int]$SampleSeconds=15
)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path
$configPath=(Resolve-Path -LiteralPath $Config).Path
if(-not $DiagnosticExecutable){$DiagnosticExecutable=Join-Path $runtime 'usbbridge.exe'}
$DiagnosticExecutable=(Resolve-Path -LiteralPath $DiagnosticExecutable).Path
$output=[IO.Path]::GetFullPath($OutputDirectory)
if(Test-Path -LiteralPath (Join-Path $output 'manifest.local.json')){throw 'Use a new qualification directory; existing evidence is not overwritten.'}
New-Item -ItemType Directory -Path $output -Force|Out-Null
$stopFile=Join-Path $output 'stop.request'
$resultFile=Join-Path $output 'result.local.json'
$heartbeat=Join-Path $output 'heartbeat.local.json'
$logFile=Join-Path $output 'samples.local.jsonl'
$seconds=[int][Math]::Ceiling($Hours*3600)
$started=[DateTime]::UtcNow;$timer=[Diagnostics.Stopwatch]::StartNew()
$manifest=[ordered]@{schema=1;role=$Role;started_utc=$started.ToString('o');requested_seconds=$seconds;camera_test=[bool]$CameraIdentity;minimum_fps=$MinimumFps;image_recording=$false;config_hash=(Get-FileHash -LiteralPath $configPath).Hash;windows_build=[Environment]::OSVersion.Version.ToString();binaries=@()}
foreach($name in @('usbbridge.exe','usbbridge-agent.exe','usbbridge-client.exe','usbbridge-server.exe')){
 $path=if($name-eq 'usbbridge.exe'){$DiagnosticExecutable}else{Join-Path $runtime $name}
 if(Test-Path -LiteralPath $path){$manifest.binaries+=@([ordered]@{file=$name;sha256=(Get-FileHash -LiteralPath $path).Hash})}
}
function Save-Json($Path,$Value){
 $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
 [IO.File]::WriteAllText($temporary,($Value|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
 Move-Item -LiteralPath $temporary -Destination $Path -Force
}
function Append-Sample($Value){
 if((Test-Path -LiteralPath $logFile)-and (Get-Item -LiteralPath $logFile).Length-ge 33554432){
  for($i=3;$i-ge 1;$i--){$old=$logFile+'.'+$i;if(Test-Path -LiteralPath $old){Move-Item -LiteralPath $old -Destination ($logFile+'.'+($i+1)) -Force}}
  Move-Item -LiteralPath $logFile -Destination ($logFile+'.1') -Force
 }
 [IO.File]::AppendAllText($logFile,(ConvertTo-Json -InputObject $Value -Depth 8 -Compress)+[Environment]::NewLine,[Text.UTF8Encoding]::new($false))
}
Save-Json (Join-Path $output 'manifest.local.json') $manifest
$child=$null;$latest=$null;$lastSample=-$SampleSeconds;$failure=$null;$interrupted=$false
$stallCount=0;$lowWindows=0;$windowStart=$null;$lastFrameChange=0.0;$lastFrameCount=0L;$readTask=$null;$errorTask=$null
try{
 if($CameraIdentity){
  $info=New-Object Diagnostics.ProcessStartInfo
  $info.FileName=$DiagnosticExecutable
  foreach($value in @($configPath,$CameraIdentity,$stopFile)){if($value.Contains('"')-or $value.Contains([char]0)){throw 'Invalid camera command argument.'}}
  $info.Arguments='--config "'+$configPath+'" '+$Role.ToLowerInvariant()+' camera-test "'+$CameraIdentity+'" --seconds '+$seconds+' --low-light off --stop-file "'+$stopFile+'"'
  $info.StandardOutputEncoding=[Text.UTF8Encoding]::new($false);$info.StandardErrorEncoding=[Text.UTF8Encoding]::new($false);$info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
  $child=New-Object Diagnostics.Process;$child.StartInfo=$info
  if(-not $child.Start()){throw 'Could not start camera test.'}
  $readTask=$child.StandardOutput.ReadLineAsync()
  $errorTask=$child.StandardError.ReadToEndAsync()
 }
 while($timer.Elapsed.TotalSeconds-lt $seconds){
  if(Test-Path -LiteralPath $stopFile){$interrupted=$true;break}
  if($child){
   if($readTask.Wait(200)){
    $line=$readTask.Result
    if($null-ne $line){
     if($line){$latest=$line|ConvertFrom-Json}
     $readTask=$child.StandardOutput.ReadLineAsync()
    }elseif($child.HasExited){break}
   }
   if($latest){
    if([long]$latest.stats.frames-ne $lastFrameCount){$lastFrameCount=[long]$latest.stats.frames;$lastFrameChange=$timer.Elapsed.TotalSeconds}
    if($timer.Elapsed.TotalSeconds-gt 30-and $timer.Elapsed.TotalSeconds-$lastFrameChange-gt 15){$stallCount++;$lastFrameChange=$timer.Elapsed.TotalSeconds}
    if($null-eq $windowStart){$windowStart=$latest}
    $delta=([double]$latest.stats.elapsed_us-[double]$windowStart.stats.elapsed_us)/1e6
    if($delta-ge 10){
     $fps=([long]$latest.stats.frames-[long]$windowStart.stats.frames)/$delta
     if($timer.Elapsed.TotalSeconds-gt 30-and $fps-lt $MinimumFps){$lowWindows++}
     $windowStart=$latest
    }
   }
   if($child.HasExited-and $null-eq $line){break}
  }else{Start-Sleep -Milliseconds 250}
  if($timer.Elapsed.TotalSeconds-$lastSample-ge $SampleSeconds){
   $lastSample=$timer.Elapsed.TotalSeconds
   $processes=@(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe' OR Name='usbbridge-server.exe' OR Name='usbbridge.exe'"|Where-Object {$_.ExecutablePath-and (([IO.Path]::GetDirectoryName($_.ExecutablePath)-eq $runtime)-or ($child-and $_.ProcessId-eq $child.Id))}|ForEach-Object{
    $process=Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
    if($process){[ordered]@{pid=$process.Id;name=$process.ProcessName;started_utc=$process.StartTime.ToUniversalTime().ToString('o');private_bytes=$process.PrivateMemorySize64;working_set_bytes=$process.WorkingSet64;handles=$process.HandleCount;threads=$process.Threads.Count;cpu_seconds=$process.TotalProcessorTime.TotalSeconds}}
   })
   $state=if(Test-Path -LiteralPath ([IO.Path]::ChangeExtension($configPath,'status.json'))){try{Get-Content -LiteralPath ([IO.Path]::ChangeExtension($configPath,'status.json')) -Raw|ConvertFrom-Json}catch{$null}}else{$null}
   # Only phase/count metadata from status; never serialize proxy settings or configuration.
   $entry=[ordered]@{utc=[DateTime]::UtcNow.ToString('o');elapsed_seconds=$timer.Elapsed.TotalSeconds;processes=$processes;camera=$latest;controller_phase=$state.phase;stalls=$stallCount;low_fps_windows=$lowWindows}
   Append-Sample $entry
   Save-Json $heartbeat ([ordered]@{schema=1;phase='running';role=$Role;monitor_pid=$PID;elapsed_seconds=$timer.Elapsed.TotalSeconds;requested_seconds=$seconds;camera_frames=$(if($latest){$latest.stats.frames}else{0});received_fps=$(if($latest){$latest.received_fps}else{$null});stalls=$stallCount;low_fps_windows=$lowWindows;updated_utc=[DateTime]::UtcNow.ToString('o')})
  }
 }
 if($child){
  if(-not $child.HasExited){[IO.File]::WriteAllText($stopFile,'stop');if(-not $child.WaitForExit(10000)){throw 'Camera test did not stop cooperatively.'}}
  if($child.ExitCode-ne 0){throw ('Camera test failed: '+$errorTask.Result)}
  if(-not $latest-or $latest.stats.frames-eq 0-or $latest.stats.decoded-eq 0){throw 'No decoded camera frames were received.'}
 }
 if($timer.Elapsed.TotalSeconds-lt $seconds-2-and -not $interrupted){throw 'Qualification ended before the requested duration.'}
 if($stallCount-gt 0-or $lowWindows-gt 0){$failure='Camera stalls or low-FPS windows were recorded.'}
}catch{$failure=$_.Exception.Message}
finally{
 if($child-and -not $child.HasExited){
  [IO.File]::WriteAllText($stopFile,'stop')
  if(-not $child.WaitForExit(10000)){$child.Kill();$failure='Camera test required forced termination; hardware restoration is not confirmed.'}
 }
 $elapsed=$timer.Elapsed.TotalSeconds
 $phase=if($interrupted){'interrupted'}elseif($failure){'failed'}else{'completed'}
 $result=[ordered]@{schema=1;phase=$phase;passed=($phase-eq 'completed');role=$Role;requested_seconds=$seconds;elapsed_seconds=$elapsed;started_utc=$started.ToString('o');finished_utc=[DateTime]::UtcNow.ToString('o');camera_test=[bool]$CameraIdentity;last_camera=$latest;stalls=$stallCount;low_fps_windows=$lowWindows;error=$failure;binaries=$manifest.binaries;config_unchanged=((Get-FileHash -LiteralPath $configPath).Hash-eq $manifest.config_hash);no_images_saved=$true}
 Save-Json $resultFile $result
 Save-Json $heartbeat $result
 if($child){$child.Dispose()}
}
if($failure){throw $failure}
