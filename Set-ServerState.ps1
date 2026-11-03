# Start/stop the selected transport while preserving device selection and autostart.
param([Parameter(Mandatory=$true)][string]$Config,[Parameter(Mandatory=$true)][string]$RequestFile,[switch]$Elevate)
$ErrorActionPreference='Stop'
$Config=[IO.Path]::GetFullPath($Config)
$RequestFile=[IO.Path]::GetFullPath($RequestFile)
$resultFile=$RequestFile+'.result.json'
function Write-Atomic($Path,$Value){
    $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    [IO.File]::WriteAllText($temporary,($Value|ConvertTo-Json -Depth 10),[Text.UTF8Encoding]::new($true))
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'Administrator required to start/stop the USB server.'}
    try{
        $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Config',('"'+$Config+'"'),'-RequestFile',('"'+$RequestFile+'"') -PassThru
        $child.WaitForExit();exit $child.ExitCode
    }catch{Write-Atomic $resultFile ([ordered]@{Success=$false;Error=$_.Exception.Message});exit 1}
}
$task=$null;$stoppedController=$false;$hostData=$null;$oldEnabled=$true
try{
    $hostData=Get-Content -LiteralPath $Config -Raw|ConvertFrom-Json
    $request=Get-Content -LiteralPath $RequestFile -Raw|ConvertFrom-Json
    if($request.enabled-isnot [bool]){throw 'Request enabled must be a boolean.'}
    $runtime=(Resolve-Path -LiteralPath $hostData.runtime).Path
    $agent=Join-Path $runtime 'usbbridge-agent.exe'
    $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName 'USBBridge.Server' -ErrorAction Stop
    if(@($task.Actions).Count-ne 1-or @($task.Actions|Where-Object {$_.Execute-ne $agent-or $_.Arguments-notlike ('*--host --config "'+$Config+'"*')}).Count){throw 'Server task belongs to another installation.'}
    $state=Get-Content -LiteralPath (Join-Path $runtime 'backend-state.json') -Raw|ConvertFrom-Json
    $backend=$state.DesiredBackend
    if($backend-notin @('Native','Usbipd')){throw 'Unknown capture module.'}
    if($hostData.PSObject.Properties.Name-contains 'enabled'){$oldEnabled=[bool]$hostData.enabled}
    $task|Stop-ScheduledTask
    $stoppedController=$true
    foreach($process in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe'"|Where-Object {$_.ExecutablePath-eq $agent-and $_.CommandLine-match '(?:^|\s)--host(?:\s|$)'-and $_.CommandLine.Contains($Config)})){
        $running=Get-Process -Id $process.ProcessId -ErrorAction SilentlyContinue
        if($running){Stop-Process -Id $running.Id -Force;$running.WaitForExit()}
    }
    & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $backend -RuntimeDirectory $runtime -Profile $hostData.profile -Stop:(-not $request.enabled)
    if(-not $?){throw 'Cannot change server state. See backend-switch-status.json.'}
    $hostData|Add-Member -NotePropertyName enabled -NotePropertyValue ([bool]$request.enabled) -Force
    Write-Atomic $Config $hostData
    $status=[ordered]@{updated_unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();server=$hostData.name;server_id=$hostData.server_id;phase=$(if($request.enabled){'recovering'}else{'stopped'});enabled=[bool]$request.enabled;backend=$backend;error=$null}
    Write-Atomic ([IO.Path]::ChangeExtension($Config,'status.json')) $status
    $task|Start-ScheduledTask
    $stoppedController=$false
    Write-Atomic $resultFile ([ordered]@{Success=$true;Enabled=[bool]$request.enabled})
}catch{
    $failure=$_.Exception.Message
    if($stoppedController){
        try{
            if(-not $oldEnabled){& (Join-Path $runtime 'Switch-Backend.ps1') -Backend $backend -RuntimeDirectory $runtime -Profile $hostData.profile -Stop}
            $hostData|Add-Member -NotePropertyName enabled -NotePropertyValue $oldEnabled -Force
            Write-Atomic $Config $hostData
            $task|Start-ScheduledTask
        }catch{$failure+='; recovery: '+$_.Exception.Message}
    }
    Write-Atomic $resultFile ([ordered]@{Success=$false;Error=$failure});exit 1
}
