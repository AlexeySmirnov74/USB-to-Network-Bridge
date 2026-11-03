# Rebind only cached, explicitly selected identities after physical replug.
param([Parameter(Mandatory=$true)][string]$RuntimeDirectory,[Parameter(Mandatory=$true)][string]$Profile)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path
. (Join-Path $PSScriptRoot 'Write-DebugEvent.ps1')
. (Join-Path $PSScriptRoot 'New-BackendMutex.ps1')
$mutex=New-BridgeBackendMutex $runtime
$owned=$false
try{$owned=$mutex.WaitOne(0)}catch [Threading.AbandonedMutexException]{$owned=$true}
if(-not $owned){Write-BridgeEvent $runtime 'exports.ensure.busy' @{};$mutex.Dispose();exit 0}
$elapsed=[Diagnostics.Stopwatch]::StartNew()
function Invoke-DeviceDriver([string[]]$Arguments,[string]$Identity){
    $output=Join-Path $runtime ('ensure-usbipd.'+[guid]::NewGuid().ToString('N'))
    $timer=[Diagnostics.Stopwatch]::StartNew()
    Write-BridgeEvent $runtime 'exports.driver.begin' @{identity=$Identity;arguments=$Arguments}
    try{
        $p=Start-Process $usbipd -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput ($output+'.stdout.tmp') -RedirectStandardError ($output+'.stderr.tmp')
        $message=[IO.File]::ReadAllText($output+'.stderr.tmp')
        Write-BridgeEvent $runtime 'exports.driver.complete' @{identity=$Identity;arguments=$Arguments;driver_pid=$p.Id;duration_ms=$timer.ElapsedMilliseconds;exit_code=$p.ExitCode;message=$message}
        if($p.ExitCode-ne 0){throw ('Selected USB driver reservation failed: '+$message)}
    }finally{foreach($suffix in @('.stdout.tmp','.stderr.tmp')){if(Test-Path -LiteralPath ($output+$suffix)){Remove-Item -LiteralPath ($output+$suffix) -Force}}}
}
function Save-State {
    $temporary=$statePath+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    $state|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $temporary -Encoding utf8
    Move-Item -LiteralPath $temporary -Destination $statePath -Force
}
try {
    $configuration=Import-PowerShellDataFile -LiteralPath $Profile
    $statePath=Join-Path $runtime 'backend-state.json'
    $previous=Get-Content -LiteralPath $statePath -Raw|ConvertFrom-Json
    $state=@{Reservations=@();ForcedByUs=@()}
    foreach($property in $previous.PSObject.Properties){$state[$property.Name]=$property.Value}
    if($state.DesiredBackend-and $state.DesiredBackend-ne $state.Backend){exit 0}
    $inventory=Join-Path $runtime 'devices.inventory'
    if(-not(Test-Path -LiteralPath $inventory)){exit 0}
    $client=Join-Path $runtime 'usbbridge-client.exe'
    $cachedText=& $client inventory $inventory|Out-String
    if($LASTEXITCODE-ne 0){throw 'Cannot read remembered identities.'}
    $values=$cachedText|ConvertFrom-Json
    $cached=@($values|ForEach-Object {$_})
    $usbipd=Join-Path (Get-ItemProperty -LiteralPath HKLM:\SOFTWARE\usbipd-win -Name APPLICATIONFOLDER).APPLICATIONFOLDER 'usbipd.exe'
    $physicalText=& $usbipd state|Out-String
    if($LASTEXITCODE-ne 0){throw 'Cannot inspect physical devices.'}
    $physical=@(($physicalText|ConvertFrom-Json).Devices)
    Write-BridgeEvent $runtime 'exports.ensure.snapshot' @{backend=$state.Backend;selected=@($cached|Where-Object {$configuration.Export-contains $_.busid}|ForEach-Object path);physical=@($physical|Select-Object BusId,InstanceId,IsForced,PersistedGuid,StubInstanceId)}
    $locations=@($physical|Where-Object BusId|ForEach-Object {[ordered]@{identity=$_.InstanceId;busid=$_.BusId}})
    $locationsFile=Join-Path $runtime 'device-locations.local.json'
    $locationsTemp=$locationsFile+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    ConvertTo-Json -InputObject $locations -Depth 5|Set-Content -LiteralPath $locationsTemp -Encoding utf8
    Move-Item -LiteralPath $locationsTemp -Destination $locationsFile -Force
    foreach($record in @($cached|Where-Object {$configuration.Export-contains $_.busid})){
        $device=@($physical|Where-Object {$_.InstanceId-eq $record.path-and $_.BusId})
        if($device.Count-ne 1){continue}
        if($state.Backend-eq 'Native'-and @($state.LocalWhenIdle)-contains $record.path){continue}
        if($state.Backend-eq 'Native'-and -not $device[0].IsForced){
            Write-BridgeEvent $runtime 'usb.rebind' @{identity=$record.path;remembered_busid=$record.busid;physical_busid=$device[0].BusId;backend='Native'}
            if(-not @($state.Reservations|Where-Object InstanceId -eq $record.path).Count){
                $state.Reservations=@($state.Reservations)+[PSCustomObject]@{BusId=$record.busid;InstanceId=$record.path;Guid=$device[0].PersistedGuid;WasShared=[bool]$device[0].PersistedGuid}
            }
            $state.ForcedByUs=@(@($state.ForcedByUs)+$record.busid|Select-Object -Unique)
            Save-State
            Invoke-DeviceDriver @('bind','--force','--busid',$device[0].BusId) $record.path
            $updatedText=& $usbipd state|Out-String
            if($LASTEXITCODE-ne 0){throw 'Cannot inspect the reconnected device.'}
            $updated=@(($updatedText|ConvertFrom-Json).Devices|Where-Object InstanceId -eq $record.path)
            if($updated.Count-eq 1){foreach($entry in @($state.Reservations|Where-Object InstanceId -eq $record.path)){$entry.Guid=$updated[0].PersistedGuid}}
            Save-State
        }elseif($state.Backend-eq 'Usbipd'-and -not $device[0].PersistedGuid){
            Invoke-DeviceDriver @('bind','--busid',$device[0].BusId) $record.path
        }
    }
}catch{Write-BridgeEvent $runtime 'exports.ensure.error' @{error=$_.Exception.Message};throw}
finally{Write-BridgeEvent $runtime 'exports.ensure.complete' @{duration_ms=$elapsed.ElapsedMilliseconds};if($owned){$mutex.ReleaseMutex()};$mutex.Dispose()}
