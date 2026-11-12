# Stop/resume one role for this Windows boot without changing autostart choices.
param([ValidateSet('Server','Client')][string]$Role,[string]$Config,[string]$RuntimeDirectory=$PSScriptRoot,[UInt32]$KeepProcessId=0,[switch]$Elevate,[switch]$Resume,[switch]$FunctionsOnly)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
function Get-QuitConfigArgument([string]$Arguments) {
 $matches=[regex]::Matches($Arguments,'(?i)(?:^|\s)(?:--config|-Config)\s+(?:"(?<quoted>[^"]+)"|(?<bare>[^\s"]+))(?=\s|$)')
 if($matches.Count-ne 1){return $null}
 $value=if($matches[0].Groups['quoted'].Success){$matches[0].Groups['quoted'].Value}else{$matches[0].Groups['bare'].Value}
 if(-not [IO.Path]::IsPathRooted($value)){return $null}
 [IO.Path]::GetFullPath($value)
}
function Test-QuitSamePath([string]$First,[string]$Second) {
 $First-and $Second-and [string]::Equals([IO.Path]::GetFullPath($First).TrimEnd('\'),[IO.Path]::GetFullPath($Second).TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)
}
function Read-QuitJson([string]$Path) {
 if(-not(Test-Path -LiteralPath $Path -PathType Leaf)){return $null}
 $item=Get-Item -LiteralPath $Path
 if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Application settings must not be a junction or symlink.'}
 $stream=[IO.File]::Open($item.FullName,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
 $reader=$null
 try {
  if([UInt64]$stream.Length-gt [UInt64]1048576){throw 'Application settings exceed the size limit.'}
  $reader=[IO.StreamReader]::new($stream,[Text.Encoding]::UTF8,$true,4096,$true)
  $reader.ReadToEnd()|ConvertFrom-Json
 }finally{if($reader){$reader.Dispose()};$stream.Dispose()}
}
function Get-QuitVideoConfigs([string]$MainConfig) {
 $prefix=[IO.Path]::GetFileNameWithoutExtension($MainConfig)-replace '\.local$',''
 $directory=[IO.Path]::GetDirectoryName($MainConfig)
 @('receive','send','local-send')|ForEach-Object {
  [pscustomobject]@{role=$_;mode=$(if($_-eq 'receive'){'receive'}else{'send'});health=('check-'+$_);config=(Join-Path $directory ($prefix+'.video-'+$_.ToLowerInvariant()+'.local.json'))}
 }
}
function Get-QuitOwnedTasks($Tasks,[string]$Role,[string]$MainConfig,[string]$Runtime,$Videos) {
 $agent=Join-Path $Runtime 'usbbridge-agent.exe';$camera=Join-Path $Runtime 'usbbridge-camera.exe'
 $mode=if($Role-eq 'Server'){'--host'}else{'--client'}
 $results=New-Object 'Collections.Generic.List[object]'
 foreach($task in @($Tasks)){
  if($task.TaskPath-ne '\USBBridge\'){continue}
  $roleTask=$task.TaskName-eq ('USBBridge.'+$Role)-or $task.TaskName-eq ('USBBridge.Health.'+$Role)-or $task.TaskName-like ('USBBridge.Tray.'+$Role+'.*')
  $actions=@($task.Actions)
  if($roleTask){
   if($actions.Count-ne 1-or -not(Test-QuitSamePath $actions[0].Execute $agent)-or -not(Test-QuitSamePath (Get-QuitConfigArgument ([string]$actions[0].Arguments)) $MainConfig)){throw 'Another installation/configuration owns this role startup task.'}
   $arguments=[string]$actions[0].Arguments
   $kind=if($task.TaskName-like ('USBBridge.Tray.'+$Role+'.*')){'tray'}elseif($task.TaskName-eq ('USBBridge.Health.'+$Role)){'health'}else{'main'}
   $valid=if($kind-eq 'tray'){$arguments-match ('(?:^|\s)--tray(?:\s|$)')-and $arguments-match ('(?:^|\s)--role\s+'+$Role+'(?:\s|$)')}else{$arguments-match ('(?:^|\s)'+$mode+'(?:\s|$)')-and $arguments-match $(if($kind-eq 'health'){'(?:^|\s)--check-supervisor(?:\s|$)'}else{'(?:^|\s)--supervise(?:\s|$)'})}
   if(-not $valid){throw 'The role startup task action is not recognized.'}
   $results.Add([pscustomobject]@{task=$task;kind=$kind;config=$MainConfig;enabled=($task.State-ne 'Disabled');running=($task.State-eq 'Running')})
   continue
  }
  foreach($video in $Videos){
   $selected=@($actions|Where-Object {Test-QuitSamePath (Get-QuitConfigArgument ([string]$_.Arguments)) $video.config})
   if(-not $selected.Count){continue}
   if($actions.Count-ne 1-or -not(Test-QuitSamePath $actions[0].Execute $camera)){throw 'An unexpected startup task references an owned video configuration.'}
   $suffix=if($video.role-eq 'local-send'){'LocalSend'}elseif($video.role-eq 'receive'){'Receive'}else{'Send'}
   $isMain=$task.TaskName-eq ('USBBridge.Video.'+$suffix);$isHealth=$task.TaskName-eq ('USBBridge.Video.Health.'+$suffix)
   $expected=if($isHealth){$video.health}else{$video.mode}
   if((-not $isMain-and -not $isHealth)-or [string]$actions[0].Arguments-notmatch ('^'+$expected+'\s+--config\s+')){throw 'The owned video task action is not recognized.'}
   $results.Add([pscustomobject]@{task=$task;kind=$(if($isHealth){'video-health'}else{'video'});config=$video.config;enabled=($task.State-ne 'Disabled');running=($task.State-eq 'Running')})
  }
 }
 @($results.ToArray())
}
function Get-QuitNamedAbsoluteArgument([string]$Arguments,[string]$Name) {
 $matches=[regex]::Matches($Arguments,('(?i)(?:^|\s)-'+[regex]::Escape($Name)+'\s+(?:"(?<quoted>[^"]+)"|(?<bare>[^\s"]+))(?=\s|$)'))
 if($matches.Count-ne 1){return $null}
 $value=if($matches[0].Groups['quoted'].Success){$matches[0].Groups['quoted'].Value}else{$matches[0].Groups['bare'].Value}
 if(-not [IO.Path]::IsPathRooted($value)-or $value-match '^[A-Za-z]:(?:$|[^\\/])'){return $null}
 [IO.Path]::GetFullPath($value)
}
function Get-QuitProcessKind($Process,[string]$Role,[string]$MainConfig,[string]$Runtime,$Videos,$BackendContext) {
 $selected=Get-QuitConfigArgument ([string]$Process.CommandLine)
 $arguments=[string]$Process.CommandLine
 $hasConfig=$arguments-match '(?:^|\s)(?:--config|-Config)(?:\s|$)'
 $root=if((Split-Path -Leaf $Runtime)-eq 'app'){Split-Path -Parent $Runtime}else{$Runtime}
 if(Test-QuitSamePath $Process.ExecutablePath (Join-Path $root ('USB-to-Network-Bridge-'+$Role+'.exe'))){
  if($hasConfig-and -not(Test-QuitSamePath $selected $MainConfig)){return $null}
  return 'launcher'
 }
 $powershell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
 if(Test-QuitSamePath $Process.ExecutablePath $powershell){
  $files=[regex]::Matches($arguments,'(?i)(?:^|\s)-File\s+(?:"(?<quoted>[^"]+)"|(?<bare>[^\s"]+))(?=\s|$)')
  if($files.Count-ne 1){return $null}
  $file=if($files[0].Groups['quoted'].Success){$files[0].Groups['quoted'].Value}else{$files[0].Groups['bare'].Value}
  if(-not [IO.Path]::IsPathRooted($file)-or $file-match '^[A-Za-z]:(?:$|[^\\/])'){return $null}
  if($Role-eq 'Server'-and $BackendContext-and (Test-QuitSamePath (Get-QuitNamedAbsoluteArgument $arguments 'RuntimeDirectory') $Runtime)){
   if((Test-QuitSamePath $file (Join-Path $Runtime 'Switch-Backend.ps1'))-or (Test-QuitSamePath $file (Join-Path $Runtime 'Ensure-Exports.ps1'))){
    if(Test-QuitSamePath (Get-QuitNamedAbsoluteArgument $arguments 'Profile') $BackendContext.profile){return 'backend-helper'}
    return $null
   }
   if((Test-QuitSamePath $file (Join-Path $Runtime 'Native-Capture.ps1'))-and $BackendContext.native_verified){
    $requestPath=Get-QuitNamedAbsoluteArgument $arguments 'RequestFile'
    if(-not $requestPath-or -not(Test-QuitSamePath ([IO.Path]::GetDirectoryName($requestPath)) $Runtime)-or [IO.Path]::GetFileName($requestPath)-notmatch '^native-capture\.([a-fA-F0-9-]{36})\.request\.tmp$'){return $null}
    try {
     [void][guid]::Parse($Matches[1]);$request=Read-QuitJson $requestPath
     if($request.operation-notin @('acquire','release')-or $request.identity-notlike 'USB\*'-or [UInt32]$request.process_id-ne [UInt32]$BackendContext.native_pid-or [UInt32]$Process.ParentProcessId-ne [UInt32]$BackendContext.native_pid){return $null}
     if($request.token){[void][guid]::Parse([string]$request.token)}elseif($request.operation-eq 'acquire'){return $null}
     return 'backend-helper'
    }catch{return $null}
   }
  }
  $roles=[regex]::Matches($arguments,'(?i)(?:^|\s)-Role\s+(?:"(?<quoted>Server|Client)"|(?<bare>Server|Client))(?=\s|$)')
  if($roles.Count-ne 1){return $null}
  $selectedRole=if($roles[0].Groups['quoted'].Success){$roles[0].Groups['quoted'].Value}else{$roles[0].Groups['bare'].Value}
  if(-not [IO.Path]::IsPathRooted($file)-or $selectedRole-ne $Role){return $null}
  if(-not(Test-QuitSamePath $file (Join-Path $Runtime 'Open-Installation.ps1'))-and -not(Test-QuitSamePath $file (Join-Path $Runtime 'Configure-Installation.ps1'))){return $null}
  if($hasConfig-and -not(Test-QuitSamePath $selected $MainConfig)){return $null}
  return 'setup'
 }
 if(-not $selected){return $null}
 if(Test-QuitSamePath $Process.ExecutablePath (Join-Path $Runtime 'usbbridge-agent.exe')){
  if(-not(Test-QuitSamePath $selected $MainConfig)){return $null}
  $opposite=if($Role-eq 'Server'){'client'}else{'server'}
  if($arguments-match ('(?:^|\s)--'+$(if($Role-eq 'Server'){'client'}else{'host'})+'(?:\s|$)')-or $arguments-match ('(?:^|\s)--role\s+'+$opposite+'(?:\s|$)')){return $null}
  if($arguments-match '(?:^|\s)--camera-preview(?:\s|$)'){return 'preview'}
  if($arguments-match '(?:^|\s)--(?:gui|tray)(?:\s|$)'-and $arguments-match ('(?:^|\s)--role\s+'+$Role+'(?:\s|$)')){return 'ui'}
  $mode=if($Role-eq 'Server'){'host'}else{'client'}
  if($arguments-match ('(?:^|\s)--'+$mode+'(?:\s|$)')){return 'control'}
  return $null
 }
 if(Test-QuitSamePath $Process.ExecutablePath (Join-Path $Runtime 'usbbridge-camera.exe')){
  foreach($video in $Videos){
   if((Test-QuitSamePath $selected $video.config)-and $arguments-match ('(?:^|["\s])(?:'+$video.mode+'|'+$video.health+')\s+--config\s+')){return 'video'}
  }
 }
 $null
}
function ConvertTo-QuitArgument([string]$Value) {
 $escaped=[regex]::Replace($Value,'(\\*)"','$1$1\"')
 $escaped=[regex]::Replace($escaped,'(\\+)$','$1$1')
 '"'+$escaped+'"'
}
function Invoke-QuitCommand([string]$Executable,[string[]]$Arguments,[int]$TimeoutSeconds=30) {
 $start=New-Object Diagnostics.ProcessStartInfo
 $start.FileName=$Executable;$start.Arguments=(@($Arguments|ForEach-Object {ConvertTo-QuitArgument $_})-join ' ')
 $start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
 $process=New-Object Diagnostics.Process;$process.StartInfo=$start
 try {
  if(-not $process.Start()){throw 'Could not start an application cleanup command.'}
  $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
  if(-not $process.WaitForExit($TimeoutSeconds*1000)){$process.Kill();$process.WaitForExit();throw 'An application cleanup command timed out.'}
  $output=$stdout.GetAwaiter().GetResult();$errorText=$stderr.GetAwaiter().GetResult()
  if([UInt64]$output.Length-gt [UInt64]1048576-or [UInt64]$errorText.Length-gt [UInt64]1048576){throw 'An application cleanup command returned too much output.'}
  if($process.ExitCode-ne 0){throw ('An application cleanup command failed: '+$process.ExitCode+'. '+$errorText.Substring(0,[Math]::Min(2048,$errorText.Length)))}
  $output
 }finally{$process.Dispose()}
}
function Get-QuitState([string]$Cli,[string]$Config,[string]$Role) {
 $text=Invoke-QuitCommand $Cli @('--config',$Config,$Role.ToLowerInvariant(),'application-state','status')
 $value=$text|ConvertFrom-Json
 if($value.stopped-isnot [bool]){throw 'The application stop state is invalid.'}
 [bool]$value.stopped
}
function Set-QuitState([string]$Cli,[string]$Config,[string]$Role,[bool]$Stopped) {
 $action=if($Stopped){'hold'}else{'resume'}
 $null=Invoke-QuitCommand $Cli @('--config',$Config,$Role.ToLowerInvariant(),'application-state',$action)
}
function Get-QuitOwnedProcesses([string]$Role,[string]$Config,[string]$Runtime,$Videos,[UInt32]$KeepProcessId,$BackendContext) {
 $results=New-Object 'Collections.Generic.List[object]'
 foreach($item in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe' OR Name='usbbridge-camera.exe' OR Name='USB-to-Network-Bridge-Client.exe' OR Name='USB-to-Network-Bridge-Server.exe' OR Name='powershell.exe'")){
  if([UInt32]$item.ProcessId-eq $KeepProcessId){continue}
  if(Get-QuitProcessKind $item $Role $Config $Runtime $Videos $BackendContext){$results.Add($item)}
 }
 if($results.Count-gt 128){throw 'Too many owned application processes are running; cleanup was refused.'}
 @($results.ToArray())
}
function Stop-QuitOwnedProcess($Item,[string]$Role,[string]$Config,[string]$Runtime,$Videos) {
 $process=Get-Process -Id $Item.ProcessId -ErrorAction SilentlyContinue
 if(-not $process){return}
 try {
  $null=$process.Handle
  $fresh=Get-CimInstance Win32_Process -Filter ('ProcessId='+[UInt32]$Item.ProcessId) -ErrorAction SilentlyContinue
  if(-not $fresh-or -not(Get-QuitProcessKind $fresh $Role $Config $Runtime $Videos)-or -not(Test-QuitSamePath $process.Path $fresh.ExecutablePath)){return}
  if(-not $process.HasExited){$process.Kill();if(-not $process.WaitForExit(10000)){throw 'An owned application process did not exit.'}}
 }finally{$process.Dispose()}
}
function Open-QuitProcessLease($Item,[string]$Role,[string]$Config,[string]$Runtime,$Videos,$BackendContext) {
 $process=Get-Process -Id $Item.ProcessId -ErrorAction SilentlyContinue
 if(-not $process){return $null}
 $retained=$false
 try {
  $null=$process.Handle
  if($process.HasExited-or -not(Test-QuitSamePath $process.Path $Item.ExecutablePath)){return $null}
  $kind=Get-QuitProcessKind $Item $Role $Config $Runtime $Videos $BackendContext
  if(-not $kind){return $null}
  $retained=$true
  [pscustomobject]@{process=$process;item=$Item;kind=$kind;context=$BackendContext}
 }catch [ArgumentException]{return $null}catch [InvalidOperationException]{return $null}
 finally{if(-not $retained){$process.Dispose()}}
}
function Stop-QuitProcessLease($Lease,[string]$Role,[string]$Config,[string]$Runtime,$Videos) {
 $process=$Lease.process
 if($process.HasExited){return}
 # Retained handles prevent PID reuse. Recheck the current command/config too
 # before forcing a process that did not finish through its normal stop path.
 $fresh=Get-CimInstance Win32_Process -Filter ('ProcessId='+[UInt32]$process.Id) -ErrorAction SilentlyContinue
 if(-not $fresh-or -not(Get-QuitProcessKind $fresh $Role $Config $Runtime $Videos $Lease.context)-or -not(Test-QuitSamePath $process.Path $fresh.ExecutablePath)){return}
 if(-not $process.HasExited){$process.Kill();if(-not $process.WaitForExit(10000)){throw 'An owned application process did not exit.'}}
}
function Close-QuitComObject($Value) {
 if($null-ne $Value-and [Runtime.InteropServices.Marshal]::IsComObject($Value)){
  [void][Runtime.InteropServices.Marshal]::ReleaseComObject($Value)
 }
}
function New-QuitTaskSession {
 $service=$null;$folder=$null;$retained=$false
 try {
  $service=New-Object -ComObject Schedule.Service
  $service.Connect()
  try{$folder=$service.GetFolder('\USBBridge')}catch{
   if($_.Exception.GetBaseException().HResult-notin @(-2147024894,-2147024893)){throw}
  }
  $result=[pscustomobject]@{service=$service;folder=$folder}
  $retained=$true
  $result
 }finally{if(-not $retained){Close-QuitComObject $folder;Close-QuitComObject $service}}
}
function Close-QuitTaskSession($Session) {
 if($Session){Close-QuitComObject $Session.folder;Close-QuitComObject $Session.service}
}
function ConvertTo-QuitTaskSnapshot($RegisteredTask,$Session,[switch]$KeepRegisteredTask) {
 $definition=$null;$actions=$null
 try {
  $name=[string]$RegisteredTask.Name
  if(-not [string]::Equals([string]$RegisteredTask.Path,('\USBBridge\'+$name),[StringComparison]::OrdinalIgnoreCase)){throw 'An unexpected task path was returned by Task Scheduler.'}
  $definition=$RegisteredTask.Definition;$actions=$definition.Actions
  $values=New-Object 'Collections.Generic.List[object]'
  for($index=1;$index-le $actions.Count;$index++){
   $action=$null
   try {
    $action=$actions.Item($index)
    # Only executable actions can match existing role/config ownership rules.
    $execute=$null;$arguments=$null
    if([int]$action.Type-eq 0){$execute=[string]$action.Path;$arguments=[string]$action.Arguments}
    $values.Add([pscustomobject]@{Execute=$execute;Arguments=$arguments})
   }finally{Close-QuitComObject $action}
  }
  $state=switch([int]$RegisteredTask.State){0{'Unknown'}1{'Disabled'}2{'Queued'}3{'Ready'}4{'Running'}default{throw 'Task Scheduler returned an invalid task state.'}}
  [pscustomobject]@{TaskPath='\USBBridge\';TaskName=$name;State=$state;Enabled=[bool]$RegisteredTask.Enabled;Actions=@($values.ToArray());Session=$Session;RegisteredTask=$(if($KeepRegisteredTask){$RegisteredTask}else{$null})}
 }finally{Close-QuitComObject $actions;Close-QuitComObject $definition}
}
function Get-QuitTasks($Session) {
 if(-not $Session.folder){return}
 $tasks=$null
 try {
  # Include hidden tasks just as the startup-task inventory must do.
  $tasks=$Session.folder.GetTasks(1)
  for($index=1;$index-le $tasks.Count;$index++){
   $task=$null
   try{$task=$tasks.Item($index);ConvertTo-QuitTaskSnapshot $task $Session}
   finally{Close-QuitComObject $task}
  }
 }finally{Close-QuitComObject $tasks}
}
function Get-QuitUnchangedTask($Entry) {
 if($Entry.task.TaskPath-ne '\USBBridge\'-or -not $Entry.task.Session.folder){throw 'The startup task session is unavailable.'}
 $registered=$null;$retained=$false
 try {
  # Reopen the exact task and read its current definition/state immediately
  # before the operation. The initial enumeration is never an authority to stop.
  $registered=$Entry.task.Session.folder.GetTask($Entry.task.TaskName)
  $current=ConvertTo-QuitTaskSnapshot $registered $Entry.task.Session -KeepRegisteredTask
  if($current.TaskName-cne $Entry.task.TaskName-or @($current.Actions).Count-ne 1-or -not(Test-QuitSamePath $current.Actions[0].Execute $Entry.task.Actions[0].Execute)-or [string]$current.Actions[0].Arguments-cne [string]$Entry.task.Actions[0].Arguments){throw 'A startup task changed during application cleanup.'}
  $retained=$true
  $current
 }finally{if(-not $retained){Close-QuitComObject $registered}}
}
function Stop-QuitOwnedTask($Entry) {
 $current=Get-QuitUnchangedTask $Entry
 try {
  if($current.State-eq 'Running'){
   $current.RegisteredTask.Stop(0)
   return $true
  }
  $false
 }finally{Close-QuitComObject $current.RegisteredTask}
}
function Start-QuitOwnedTask($Entry) {
 $current=Get-QuitUnchangedTask $Entry
 try {
  if($Entry.enabled-and $current.Enabled-and $current.State-ne 'Disabled'){
   $instance=$null
   try{$instance=$current.RegisteredTask.Run($null)}finally{Close-QuitComObject $instance}
  }
 }finally{Close-QuitComObject $current.RegisteredTask}
}

function Invoke-QuitApplication {
 param([ValidateSet('Server','Client')][string]$Role,[string]$Config,[string]$Runtime,[UInt32]$KeepProcessId=0,[switch]$Resume)
$clock=[Diagnostics.Stopwatch]::StartNew();$phaseStarted=[Int64]0;$timings=[ordered]@{}
. (Join-Path $runtime 'FirstRun-Identity.ps1')
. (Join-Path $runtime 'Portable-Paths.ps1')
. (Join-Path $runtime 'Write-DebugEvent.ps1')
$cli=Join-Path $runtime 'usbbridge.exe';$agent=Join-Path $runtime 'usbbridge-agent.exe';$camera=Join-Path $runtime 'usbbridge-camera.exe'
foreach($path in @($cli,$agent,$camera,$Config)){if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw 'The application or its configuration is incomplete.'}}
$marker=Get-BridgeMachineMarker -Role $Role -Runtime $runtime
if(-not $marker-or $marker.schema-notin @(1,2)-or $marker.role-ne $Role-or -not $marker.initialized-or $marker.machine_id-ne (Get-BridgeMachineIdentity)-or -not(Test-QuitSamePath $marker.config $Config)){throw 'The requested configuration is not verified for this computer and role.'}
$operationLock=$null;$wasStopped=$false;$changed=$false;$owned=@();$videos=@(Get-QuitVideoConfigs $Config);$nativeProcess=$null;$resolved=$null;$serverState=$null;$backendContext=$null;$taskSession=$null
$leases=New-Object 'Collections.Generic.List[object]';$leasedIds=New-Object 'Collections.Generic.HashSet[UInt32]';$stoppedTasks=0
try {
 try{$operationLock=[IO.File]::Open([IO.Path]::ChangeExtension($Config,'application-operation.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)}catch [IO.IOException]{throw 'The application is already stopping or resuming. Retry when that operation finishes.'}
 $main=Read-QuitJson $Config
 if(-not $main){throw 'The application configuration is missing.'}
 if($Role-eq 'Server'){
  $resolved=Resolve-BridgeConfigPaths $main $Config $runtime
  if(-not(Test-QuitSamePath $resolved.runtime $runtime)){throw 'The server configuration belongs to another runtime.'}
  $resolved.profile=(Resolve-Path -LiteralPath $resolved.profile).Path
  $profileData=Import-PowerShellDataFile -LiteralPath $resolved.profile
  if($profileData.Listen-notmatch '^(\[[0-9A-Fa-f:]+\]|[0-9.]+):([0-9]{1,5})$'-or [UInt16]::Parse($Matches[2])-eq 0){throw 'The server profile listen address is invalid.'}
  [Net.IPAddress]::Parse($Matches[1].Trim('[',']'))|Out-Null
  if(@($profileData.AllowedPeers).Count-eq 0){throw 'The server profile has no allowed client addresses.'}
  foreach($peer in @($profileData.AllowedPeers)){[Net.IPAddress]::Parse($peer)|Out-Null}
  foreach($busid in @($profileData.Export)){if($busid-notmatch '^(\d+-\d+|cache-[a-f0-9]{25})$'){throw 'The server profile contains an invalid export selector.'}}
  $serverState=Read-QuitJson (Join-Path $runtime 'backend-state.json')
  if(-not $serverState-or $serverState.DesiredBackend-notin @('Native','Usbipd')){throw 'The server transport ownership state is unavailable.'}
  if($serverState.NativePid){
   $nativeProcess=Get-Process -Id ([UInt32]$serverState.NativePid) -ErrorAction SilentlyContinue
   if($nativeProcess){
    $null=$nativeProcess.Handle
    if(-not(Test-QuitSamePath $nativeProcess.Path (Join-Path $runtime 'usbbridge-server.exe'))-or -not $serverState.NativeStartedUtc-or $nativeProcess.StartTime.ToUniversalTime().ToString('o')-ne $serverState.NativeStartedUtc){throw 'The remembered Native transport process belongs to another session.'}
    if($serverState.NativeCreatedFileTime-and $nativeProcess.StartTime.ToFileTimeUtc().ToString()-ne [string]$serverState.NativeCreatedFileTime){throw 'The Native transport process identity changed.'}
   }
  }
  $backendContext=[pscustomobject]@{profile=$resolved.profile;native_verified=($null-ne $nativeProcess);native_pid=$(if($nativeProcess){[UInt32]$nativeProcess.Id}else{[UInt32]0})}
 }
 $taskSession=New-QuitTaskSession
 $tasks=@(Get-QuitTasks $taskSession)
 $owned=@(Get-QuitOwnedTasks $tasks $Role $Config $runtime $videos)
 if($KeepProcessId){
  $caller=Get-CimInstance Win32_Process -Filter ('ProcessId='+$KeepProcessId) -ErrorAction SilentlyContinue
  if(-not $caller-or (Get-QuitProcessKind $caller $Role $Config $runtime $videos)-ne 'ui'){throw 'KeepProcessId must identify this role GUI or tray process.'}
 }
 $wasStopped=Get-QuitState $cli $Config $Role
 $timings.preflight_ms=$clock.ElapsedMilliseconds;$phaseStarted=$clock.ElapsedMilliseconds
 if($Resume){
  Set-QuitState $cli $Config $Role $false;$changed=$true
  foreach($entry in @($owned|Where-Object {$_.kind-in @('main','video')})){Start-QuitOwnedTask $entry}
  Write-BridgeEvent $runtime 'application.resume' @{role=$Role;config=$Config;tasks=@($owned|Where-Object kind -in @('main','video')|ForEach-Object {$_.task.TaskName})}
  [pscustomobject]@{success=$true;role=$Role;resumed=$true;autostart_changed=$false}|ConvertTo-Json -Compress
  return
 }
 Set-QuitState $cli $Config $Role $true;$changed=$true
 $timings.hold_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 # Release only the receiver evidenced by an exact owned task/process config.
 $processes=@(Get-QuitOwnedProcesses $Role $Config $runtime $videos $KeepProcessId $backendContext)
 foreach($video in @($videos|Where-Object role -eq 'receive')){
  $evidence=@($owned|Where-Object {$_.kind-eq 'video'-and (Test-QuitSamePath $_.config $video.config)}).Count-or @($processes|Where-Object {Test-QuitSamePath (Get-QuitConfigArgument ([string]$_.CommandLine)) $video.config}).Count
  if($evidence-and (Test-Path -LiteralPath $video.config)){
   $receiver=Read-QuitJson $video.config
   $port=[UInt16]$receiver.usb_port;if(-not $port){throw 'The owned virtual camera port is invalid.'}
   $null=Invoke-QuitCommand $camera @('detach-local',('127.0.0.1:'+$port))
  }
 }
 $timings.detach_receiver_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 foreach($item in $processes){
  $lease=Open-QuitProcessLease $item $Role $Config $runtime $videos $backendContext
  if($lease){$leases.Add($lease);[void]$leasedIds.Add([UInt32]$item.ProcessId)}
 }
 $timings.open_process_leases_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 if($Role-eq 'Server'){
  # Scheduled task termination can close a descendant process job. Drain the
  # backend mutex before stopping any task that might own a driver helper.
  & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $serverState.DesiredBackend -RuntimeDirectory $runtime -Profile $resolved.profile -Stop|Out-Null
  if(-not $?){throw 'The owned USB server transport did not stop.'}
  if($nativeProcess-and -not $nativeProcess.HasExited){throw 'The verified Native transport is still running.'}
  $timings.release_usb_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 }
 # The coordinator now owns termination after the driver drain. Supervisors
 # remain parked to keep their descendant jobs alive; a grace sleep cannot help.
 $timings.cooperative_wait_ms=0;$phaseStarted=$clock.ElapsedMilliseconds
 foreach($entry in $owned){
  if($KeepProcessId-and $entry.kind-eq 'tray'){continue}
  if(Stop-QuitOwnedTask $entry){$stoppedTasks++}
 }
 $timings.stop_tasks_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 # One final discovery catches a launch already in flight at hold time; no
 # repeated global CIM polling is needed while retained handles wait or exit.
 foreach($item in @(Get-QuitOwnedProcesses $Role $Config $runtime $videos $KeepProcessId $backendContext)){
  if($leasedIds.Contains([UInt32]$item.ProcessId)){continue}
  $lease=Open-QuitProcessLease $item $Role $Config $runtime $videos $backendContext
  if($lease){$leases.Add($lease);[void]$leasedIds.Add([UInt32]$item.ProcessId)}
 }
 foreach($lease in @($leases|Where-Object kind -ne 'backend-helper')){Stop-QuitProcessLease $lease $Role $Config $runtime $videos}
 $timings.stop_processes_ms=$clock.ElapsedMilliseconds-$phaseStarted;$phaseStarted=$clock.ElapsedMilliseconds
 if($Role-eq 'Client'){
  $original=[IO.File]::ReadAllBytes($Config)
  try {
   $seen=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
   foreach($server in @($main)+@($main.servers)){
    if($server-and $server.server){
     $selector=if($server.server_id){[string]$server.server_id}else{[string]$server.server}
     if($seen.Add($selector)){$null=Invoke-QuitCommand $cli @('--config',$Config,'client','pause',$selector)}
    }
   }
  }finally{[IO.File]::WriteAllBytes($Config,$original)}
  $timings.release_usb_ms=$clock.ElapsedMilliseconds-$phaseStarted
 }
 $phaseStarted=$clock.ElapsedMilliseconds
 # Backend helpers may own driver transactions. Only after Switch -Stop has
 # drained the backend mutex may a leftover gated helper be forced to exit.
 foreach($lease in @($leases|Where-Object kind -eq 'backend-helper')){Stop-QuitProcessLease $lease $Role $Config $runtime $videos}
 $remaining=@(Get-QuitOwnedProcesses $Role $Config $runtime $videos $KeepProcessId $backendContext)
 foreach($item in $remaining){
  if((Get-QuitProcessKind $item $Role $Config $runtime $videos $backendContext)-ne 'backend-helper'){throw 'Owned application processes are still stopping.'}
  if($leasedIds.Contains([UInt32]$item.ProcessId)){continue}
  $lease=Open-QuitProcessLease $item $Role $Config $runtime $videos $backendContext
  if($lease){$leases.Add($lease);[void]$leasedIds.Add([UInt32]$item.ProcessId);Stop-QuitProcessLease $lease $Role $Config $runtime $videos}
 }
 if(@($leases|Where-Object {-not $_.process.HasExited}).Count){throw 'An owned process could not be verified as stopped.'}
 $timings.verify_ms=$clock.ElapsedMilliseconds-$phaseStarted;$timings.total_ms=$clock.ElapsedMilliseconds
 Write-BridgeEvent $runtime 'application.quit' @{role=$Role;config=$Config;autostart_changed=$false;tasks=@($owned|ForEach-Object {$_.task.TaskName});stopped_tasks=$stoppedTasks;retained_processes=$leases.Count;timings=$timings}
 [pscustomobject]@{success=$true;role=$Role;stopped=$true;autostart_changed=$false}|ConvertTo-Json -Compress
}catch{
 $failure=$_
 if($changed){
  try{Set-QuitState $cli $Config $Role $wasStopped}catch{}
  if(-not $wasStopped){foreach($entry in @($owned|Where-Object {$_.enabled-and $_.running})){try{Start-QuitOwnedTask $entry}catch{}}}
 }
 $timings.total_ms=$clock.ElapsedMilliseconds
 Write-BridgeEvent $runtime 'application.quit.failed' @{role=$Role;error=$failure.Exception.Message;resuming=[bool]$Resume;timings=$timings}
 throw $failure
}finally{Close-QuitTaskSession $taskSession;foreach($lease in $leases){$lease.process.Dispose()};if($nativeProcess){$nativeProcess.Dispose()};if($operationLock){$operationLock.Dispose()}}

}
if($FunctionsOnly){return}
if(-not $Role-or -not $Config){throw 'Role and Config are required.'}
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path;$Config=[IO.Path]::GetFullPath($Config)
if(-not(Test-QuitSamePath $runtime $PSScriptRoot)){throw 'Run application cleanup from the application runtime directory.'}
$administrator=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $administrator){
 if(-not $Elevate){throw 'Administrator rights are required to stop or resume the USB application.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',(ConvertTo-QuitArgument $PSCommandPath),'-Role',$Role,'-Config',(ConvertTo-QuitArgument $Config),'-RuntimeDirectory',(ConvertTo-QuitArgument $runtime),'-KeepProcessId',$KeepProcessId)
 if($Resume){$arguments+='-Resume'}
 $child=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $child.WaitForExit()
 if($child.ExitCode-ne 0){throw 'Application cleanup or resume failed.'}
 return
}
Invoke-QuitApplication -Role $Role -Config $Config -Runtime $runtime -KeepProcessId $KeepProcessId -Resume:$Resume
