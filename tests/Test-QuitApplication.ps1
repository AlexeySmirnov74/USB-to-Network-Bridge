param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot),[string]$PathsSourceDirectory=$SourceDirectory)
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Quit-Application.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge exit fixture '+[guid]::NewGuid().ToString('N'))
$app=Join-Path $testRoot 'app';$mainConfig=Join-Path $app 'client.local.json';$hostConfig=Join-Path $app 'host.local.json'
function Assert-QuitFixture($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Application exit assertion failed: '+$Name)}}
function New-QuitFixtureTask([string]$Name,[string]$Exe,[string]$Arguments,[string]$State='Running'){
 [pscustomobject]@{TaskPath='\USBBridge\';TaskName=$Name;State=$State;Enabled=($State-ne 'Disabled');Actions=@([pscustomobject]@{Execute=$Exe;Arguments=$Arguments})}
}
function New-QuitFixtureCollection($Items){
 $collection=[pscustomobject]@{Count=@($Items).Count;Items=@($Items)}
 $collection|Add-Member -MemberType ScriptMethod -Name Item -Value {param($Index) $this.Items[$Index-1]}
 $collection
}
function New-QuitFixtureRegisteredTask($Task){
 $actionCollection=New-QuitFixtureCollection @($Task.Actions|ForEach-Object {[pscustomobject]@{Type=0;Path=$_.Execute;Arguments=$_.Arguments}})
 $stateCode=@{Unknown=0;Disabled=1;Queued=2;Ready=3;Running=4}[[string]$Task.State]
 $registered=[pscustomobject]@{Name=$Task.TaskName;Path=($Task.TaskPath+$Task.TaskName);State=$stateCode;Enabled=[bool]$Task.Enabled;Definition=[pscustomobject]@{Actions=$actionCollection}}
 $registered|Add-Member -MemberType ScriptMethod -Name Stop -Value {param($Flags) $global:QuitFixture.stops.Add($this.Name);$global:QuitFixture.trace.Add('stop:'+$this.Name)}
 $registered|Add-Member -MemberType ScriptMethod -Name Run -Value {param($Parameters) $global:QuitFixture.starts.Add($this.Name);$null}
 $registered
}
try {
 New-Item -ItemType Directory -Path $app -Force|Out-Null
 Copy-Item -LiteralPath (Join-Path $PathsSourceDirectory 'Portable-Paths.ps1') -Destination (Join-Path $app 'Portable-Paths.ps1')
 [IO.File]::WriteAllText($mainConfig,'{"version":1,"server":"192.0.2.25:3240","server_id":null,"devices":[],"servers":[]}')
 foreach($name in @('usbbridge.exe','usbbridge-agent.exe','usbbridge-camera.exe')){[IO.File]::WriteAllText((Join-Path $app $name),'fixture-executable-not-run')}
 $agent=Join-Path $app 'usbbridge-agent.exe';$camera=Join-Path $app 'usbbridge-camera.exe';$videos=@(Get-QuitVideoConfigs $mainConfig)
 $serverVideos=@(Get-QuitVideoConfigs $hostConfig)
 $client=New-QuitFixtureTask 'USBBridge.Client' $agent ('--supervise --client --config "'+$mainConfig+'"')
 $health=New-QuitFixtureTask 'USBBridge.Health.Client' $agent ('--check-supervisor --client --config "'+$mainConfig+'"') 'Ready'
 $tray=New-QuitFixtureTask 'USBBridge.Tray.Client.S-1-fixture' $agent ('--tray --role client --config "'+$mainConfig+'"') 'Ready'
 $server=New-QuitFixtureTask 'USBBridge.Server' $agent ('--supervise --host --config "'+$hostConfig+'"')
 $sendOther=New-QuitFixtureTask 'USBBridge.Video.Send' $camera ('send --config "'+$serverVideos[1].config+'"')
 $receive=New-QuitFixtureTask 'USBBridge.Video.Receive' $camera ('receive --config "'+$videos[0].config+'"')
 $owned=@(Get-QuitOwnedTasks @($client,$health,$tray,$server,$sendOther,$receive) Client $mainConfig $app $videos)
 Assert-QuitFixture $owned.Count 4 'only selected role and its video tasks owned'
 Assert-QuitFixture (@($owned|Where-Object {$_.task.TaskName-eq 'USBBridge.Server'}).Count) 0 'other role supervisor excluded'
 Assert-QuitFixture (@($owned|Where-Object {$_.task.TaskName-eq 'USBBridge.Video.Send'}).Count) 0 'other role video excluded'
 $bad=New-QuitFixtureTask 'USBBridge.Client' (Join-Path $testRoot 'other\usbbridge-agent.exe') ('--supervise --client --config "'+$mainConfig+'"')
 $blocked=$false;try{$null=Get-QuitOwnedTasks @($bad) Client $mainConfig $app $videos}catch{$blocked=$true}
 Assert-QuitFixture $blocked $true 'other installation role task rejected'
 $bad=New-QuitFixtureTask 'USBBridge.Client' $agent ('--supervise --client --config "'+$mainConfig+'" --config "'+$hostConfig+'"')
 $blocked=$false;try{$null=Get-QuitOwnedTasks @($bad) Client $mainConfig $app $videos}catch{$blocked=$true}
 Assert-QuitFixture $blocked $true 'ambiguous configuration task rejected'
 $ui=[pscustomobject]@{ExecutablePath=$agent;CommandLine=('"'+$agent+'" --tray --role client --config "'+$mainConfig+'"')}
 Assert-QuitFixture (Get-QuitProcessKind $ui Client $mainConfig $app $videos) 'ui' 'tray GUI recognized'
 Assert-QuitFixture (Get-QuitProcessKind $ui Server $hostConfig $app $serverVideos) $null 'other role UI excluded'
 $ui.CommandLine='"'+$agent+'" --supervise --client --config "'+$mainConfig+'"'
 Assert-QuitFixture (Get-QuitProcessKind $ui Client $mainConfig $app $videos) 'control' 'supervisor not eligible as kept UI'
 $launcher=[pscustomobject]@{ExecutablePath=(Join-Path $testRoot 'USB-to-Network-Bridge-Client.exe');CommandLine=('"'+(Join-Path $testRoot 'USB-to-Network-Bridge-Client.exe')+'"')}
 Assert-QuitFixture (Get-QuitProcessKind $launcher Client $mainConfig $app $videos) 'launcher' 'owned default role launcher recognized'
 $launcher.CommandLine+=' --config "'+$hostConfig+'"'
 Assert-QuitFixture (Get-QuitProcessKind $launcher Client $mainConfig $app $videos) $null 'launcher using another config excluded'
 $setup=[pscustomobject]@{ExecutablePath=(Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe');CommandLine=('-NoProfile -File "'+(Join-Path $app 'Configure-Installation.ps1')+'" -Role Client -Config "'+$mainConfig+'"')}
 Assert-QuitFixture (Get-QuitProcessKind $setup Client $mainConfig $app $videos) 'setup' 'owned setup PowerShell recognized'
 $setup.CommandLine=$setup.CommandLine.Replace('Configure-Installation.ps1','Quit-Application.ps1')
 Assert-QuitFixture (Get-QuitProcessKind $setup Client $mainConfig $app $videos) $null 'cleanup and unrelated PowerShell excluded'
 $backendContext=[pscustomobject]@{profile=(Join-Path $app 'server.psd1');native_verified=$true;native_pid=[UInt32]4567}
 $backend=[pscustomobject]@{ExecutablePath=$setup.ExecutablePath;CommandLine=('-NoProfile -File "'+(Join-Path $app 'Switch-Backend.ps1')+'" -RuntimeDirectory "'+$app+'" -Profile "'+$backendContext.profile+'"');ParentProcessId=[UInt32]4567}
 Assert-QuitFixture (Get-QuitProcessKind $backend Server $hostConfig $app $serverVideos $backendContext) 'backend-helper' 'server backend helper bound to runtime and profile'
 Assert-QuitFixture (Get-QuitProcessKind $backend Client $mainConfig $app $videos $backendContext) $null 'server backend helper excluded from client exit'
 $requestPath=Join-Path $app ('native-capture.'+[guid]::NewGuid().ToString()+'.request.tmp')
 [IO.File]::WriteAllText($requestPath,('{"operation":"acquire","identity":"USB\\VID_1234&PID_5678\\fixture","process_id":4567,"token":"'+[guid]::NewGuid().ToString()+'"}'))
 $backend.CommandLine='-NoProfile -File "'+(Join-Path $app 'Native-Capture.ps1')+'" -RuntimeDirectory "'+$app+'" -RequestFile "'+$requestPath+'"'
 Assert-QuitFixture (Get-QuitProcessKind $backend Server $hostConfig $app $serverVideos $backendContext) 'backend-helper' 'capture helper requires retained native owner and bounded request'
 $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Quit-Application.ps1'))
 Assert-QuitFixture ($source-match '(?im)^\s*(?:Disable|Enable|Register|Unregister)-ScheduledTask\b') $false 'exit does not rewrite autostart task state'
 $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("Write-Output '{`"stopped`":false}'"))
 $output=Invoke-QuitCommand (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') @('-NoProfile','-NonInteractive','-EncodedCommand',$encoded)
 Assert-QuitFixture ($output|ConvertFrom-Json).stopped $false 'bounded child command returns expected stdout'
 $stub=@'
function Get-BridgeMachineMarker {param($Role,$Runtime) [pscustomobject]@{schema=2;role=$Role;initialized=$true;machine_id='fixture-machine';config=$global:QuitFixture.config}}
function Get-BridgeMachineIdentity {'fixture-machine'}
'@
 [IO.File]::WriteAllText((Join-Path $app 'FirstRun-Identity.ps1'),$stub,[Text.UTF8Encoding]::new($true))
 [IO.File]::WriteAllText((Join-Path $app 'Write-DebugEvent.ps1'),'function Write-BridgeEvent {param($Runtime,$Event,$Fields) $global:QuitFixture.last_event=$Event; $global:QuitFixture.last_fields=$Fields}')
 $global:QuitFixture=@{config=$mainConfig;tasks=@($client,$health,$tray);stopped=$false;starts=(New-Object 'Collections.Generic.List[string]');stops=(New-Object 'Collections.Generic.List[string]');trace=(New-Object 'Collections.Generic.List[string]');global_queries=0;fail_pause=$true}
 function New-QuitTaskSession {
  $folder=[pscustomobject]@{}
  $folder|Add-Member -MemberType ScriptMethod -Name GetTasks -Value {param($Flags) New-QuitFixtureCollection @($global:QuitFixture.tasks|ForEach-Object {New-QuitFixtureRegisteredTask $_})}
  $folder|Add-Member -MemberType ScriptMethod -Name GetTask -Value {param($Name) $task=@($global:QuitFixture.tasks|Where-Object TaskName -eq $Name);if($task.Count-ne 1){throw 'Fixture task disappeared.'};New-QuitFixtureRegisteredTask $task[0]}
  [pscustomobject]@{service=$null;folder=$folder}
 }
 function Close-QuitTaskSession {param($Session)}
 function Get-CimInstance {param($ClassName,$Filter,$ErrorAction) if($Filter-match "Name='"){$global:QuitFixture.global_queries++}; @()}
 function Invoke-QuitCommand {
  param($Executable,$Arguments,$TimeoutSeconds)
  if($Arguments-contains 'status'){return ('{"stopped":'+$global:QuitFixture.stopped.ToString().ToLowerInvariant()+'}')}
  if($Arguments-contains 'hold'){$global:QuitFixture.stopped=$true;$global:QuitFixture.trace.Add('hold');return '{}'}
  if($Arguments-contains 'resume'){$global:QuitFixture.stopped=$false;return '{}'}
  if($Arguments-contains 'detach-local'){$global:QuitFixture.trace.Add('detach');return '{}'}
  if($Arguments-contains 'pause'){
   [IO.File]::WriteAllText($global:QuitFixture.config,'{"fixture":"temporary paused settings"}')
   if($global:QuitFixture.fail_pause){throw 'fixture pause failure'}
   return '{}'
  }
  throw 'Unexpected fixture command.'
 }
 $session=New-QuitTaskSession
 $freshOwned=@(Get-QuitOwnedTasks @(Get-QuitTasks $session) Client $mainConfig $app $videos)
 $freshEntry=@($freshOwned|Where-Object kind -eq 'main')[0]
 $originalArguments=$client.Actions[0].Arguments;$client.Actions[0].Arguments+=' --fixture-changed'
 $blocked=$false;try{$null=Stop-QuitOwnedTask $freshEntry}catch{$blocked=$true}
 Assert-QuitFixture $blocked $true 'fresh COM action is revalidated before stop'
 $client.Actions[0].Arguments=$originalArguments;$client.State='Ready'
 $null=Stop-QuitOwnedTask $freshEntry
 Assert-QuitFixture $global:QuitFixture.stops.Count 0 'fresh Ready state avoids stop even when snapshot was Running'
 $client.Enabled=$false
 Start-QuitOwnedTask $freshEntry
 Assert-QuitFixture $global:QuitFixture.starts.Count 0 'fresh disabled preference prevents task resume'
 $client.Enabled=$true;$client.State='Running';Close-QuitTaskSession $session
 $digest=(Get-FileHash -LiteralPath $mainConfig).Hash;$blocked=$false
 try{$null=Invoke-QuitApplication -Role Client -Config $mainConfig -Runtime $app}catch{$blocked=$true}
 Assert-QuitFixture $blocked $true 'partial cleanup failure reported'
 Assert-QuitFixture $global:QuitFixture.stopped $false 'partial cleanup clears newly created stop marker'
 Assert-QuitFixture (Get-FileHash -LiteralPath $mainConfig).Hash $digest 'partial cleanup restores original config bytes'
 Assert-QuitFixture ($global:QuitFixture.starts.Contains('USBBridge.Client')) $true 'partial cleanup resumes previously running controller'
 Assert-QuitFixture ($global:QuitFixture.starts.Contains('USBBridge.Health.Client')) $false 'rollback does not start previously idle health task'
 $global:QuitFixture.fail_pause=$false;$global:QuitFixture.starts.Clear();$global:QuitFixture.stops.Clear()
 $null=Invoke-QuitApplication -Role Client -Config $mainConfig -Runtime $app
 Assert-QuitFixture $global:QuitFixture.stopped $true 'successful cleanup keeps boot-bound stop marker'
 Assert-QuitFixture (Get-FileHash -LiteralPath $mainConfig).Hash $digest 'successful cleanup preserves configuration bytes'
 Assert-QuitFixture $global:QuitFixture.starts.Count 0 'successful quit does not resume tasks'
 Assert-QuitFixture $global:QuitFixture.stops.Count 1 'idle health and tray tasks do not incur a stop RPC'
 Assert-QuitFixture ($global:QuitFixture.last_fields.timings.cooperative_wait_ms-lt 100) $true 'no process means no fixed grace delay'
 Assert-QuitFixture ($global:QuitFixture.last_fields.timings.total_ms-ge 0) $true 'phase and total timings available'
 $global:QuitFixture.tasks=@($client,$health,$tray,$receive,$server,$sendOther)
 [IO.File]::WriteAllText($videos[0].config,'{"enabled":true,"usb_port":13250}')
 $global:QuitFixture.trace.Clear();$global:QuitFixture.stops.Clear();$global:QuitFixture.global_queries=0
 $null=Invoke-QuitApplication -Role Client -Config $mainConfig -Runtime $app
 Assert-QuitFixture ($global:QuitFixture.trace.IndexOf('hold')-lt $global:QuitFixture.trace.IndexOf('detach')) $true 'hold precedes explicit receiver detach'
 Assert-QuitFixture ($global:QuitFixture.trace.IndexOf('detach')-lt $global:QuitFixture.trace.IndexOf('stop:USBBridge.Video.Receive')) $true 'receiver detached before task termination'
 Assert-QuitFixture $global:QuitFixture.global_queries 3 'process discovery has bounded snapshots rather than repeated global polling'
 Assert-QuitFixture ($global:QuitFixture.stops.Contains('USBBridge.Server')) $false 'other role task remains untouched in fast path'
 Assert-QuitFixture ($global:QuitFixture.stops.Contains('USBBridge.Video.Send')) $false 'other role sender remains untouched in fast path'
 [IO.File]::WriteAllText($hostConfig,'{"runtime":"@app","profile":"server.psd1","enabled":true}')
 [IO.File]::WriteAllText((Join-Path $app 'server.psd1'),"@{ Listen='127.0.0.1:3240'; AllowedPeers=@('192.0.2.211'); Export=@() }")
 [IO.File]::WriteAllText((Join-Path $app 'backend-state.json'),'{"DesiredBackend":"Native"}')
 [IO.File]::WriteAllText((Join-Path $app 'Switch-Backend.ps1'),'param($Backend,$RuntimeDirectory,$Profile,[switch]$Stop) $global:QuitFixture.trace.Add(''release-backend'')')
 $global:QuitFixture.config=$hostConfig;$global:QuitFixture.tasks=@($server);$global:QuitFixture.trace.Clear()
 $null=Invoke-QuitApplication -Role Server -Config $hostConfig -Runtime $app
 Assert-QuitFixture ($global:QuitFixture.trace.IndexOf('release-backend')-lt $global:QuitFixture.trace.IndexOf('stop:USBBridge.Server')) $true 'server driver transactions drain before controller task job terminates'
 $global:QuitFixture.config=$mainConfig
 $global:QuitFixture.starts.Clear()
 $disabledVideo=New-QuitFixtureTask 'USBBridge.Video.Receive' $camera ('receive --config "'+$videos[0].config+'"') 'Disabled'
 $global:QuitFixture.tasks=@($client,$health,$tray,$disabledVideo)
 $null=Invoke-QuitApplication -Role Client -Config $mainConfig -Runtime $app -Resume
 Assert-QuitFixture $global:QuitFixture.stopped $false 'manual resume clears current boot hold'
 Assert-QuitFixture $global:QuitFixture.starts.Count 1 'resume starts only enabled main and video tasks'
 Assert-QuitFixture $global:QuitFixture.starts[0] 'USBBridge.Client' 'resume leaves tray and health launch to normal schedule'
 $lock=[IO.File]::Open([IO.Path]::ChangeExtension($mainConfig,'application-operation.lock'),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
 try{$blocked=$false;try{$null=Invoke-QuitApplication -Role Client -Config $mainConfig -Runtime $app -Resume}catch{$blocked=$true};Assert-QuitFixture $blocked $true 'simultaneous exit and resume rejected'}finally{$lock.Dispose()}
 foreach($file in @('Quit-Application.ps1','Restart-AgentSupervisor.ps1','Restart-VideoEndpoint.ps1')){
  $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory $file),[ref]$tokens,[ref]$errors)
  Assert-QuitFixture $errors.Count 0 ('WinPS parser '+$file)
 }
 [pscustomobject]@{passed=$true;scope='role exit ownership; rollback; resume; COM task validation';assertions=45}|ConvertTo-Json -Compress
}finally{
 Remove-Variable -Name QuitFixture -Scope Global -ErrorAction SilentlyContinue
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
