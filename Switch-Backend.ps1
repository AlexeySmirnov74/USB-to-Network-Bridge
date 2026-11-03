param(
    [Parameter(Mandatory=$true)][ValidateSet('Native','Usbipd')][string]$Backend,
    [string]$RuntimeDirectory, [string]$Profile, [switch]$Elevate, [switch]$Stop
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
if(-not $RuntimeDirectory){$RuntimeDirectory=if(Test-Path (Join-Path $PSScriptRoot 'usbbridge-server.exe')){$PSScriptRoot}else{Join-Path $PSScriptRoot 'production'}}
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path
if(-not $Profile){$Profile=Join-Path $runtime 'server.local.psd1'}
$profilePath=(Resolve-Path -LiteralPath $Profile).Path
$configuration=Import-PowerShellDataFile -LiteralPath $profilePath
if($configuration.Listen -notmatch '^(\[[0-9A-Fa-f:]+\]|[0-9.]+):([0-9]{1,5})$'){throw 'The switching helper requires an IP literal and TCP port.'}
$tcpPort=[uint16]::Parse($Matches[2]);if($tcpPort-eq 0){throw 'TCP port must be in 1..65535.'}
[System.Net.IPAddress]::Parse(($Matches[1].Trim('[',']')))|Out-Null
foreach($busid in @($configuration.Export)){if($busid-notmatch '^(\d+-\d+|cache-[a-f0-9]{25})$'){throw 'List explicit USB cache slots in Export.'}}
if(@($configuration.AllowedPeers).Count-eq 0){throw 'At least one allowed client IP is required.'}
foreach($address in @($configuration.AllowedPeers)){[System.Net.IPAddress]::Parse($address)|Out-Null}
$statePath=Join-Path $runtime 'backend-state.json'
$statusPath=Join-Path $runtime 'backend-switch-status.json'
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'Run as Administrator, or pass -Elevate for UAC.'}
    $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Backend',$Backend,'-RuntimeDirectory',('"'+$runtime+'"'),'-Profile',('"'+$profilePath+'"'))
    if($Stop){$arguments+='-Stop'}
    $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
    $child.WaitForExit()
    if(Test-Path -LiteralPath $statusPath){Get-Content -LiteralPath $statusPath -Raw}
    if($child.ExitCode-ne 0){throw 'Switching failed; see backend-switch-status.json.'}
    exit 0
}
. (Join-Path $PSScriptRoot 'New-BackendMutex.ps1')
$mutex=New-BridgeBackendMutex $runtime
$owned=$false
try{$owned=$mutex.WaitOne(45000)}catch [Threading.AbandonedMutexException]{$owned=$true}
if(-not $owned){$mutex.Dispose();throw 'Another backend change is still in progress.'}
function Write-JsonFile($Path,$Value){
    $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    ConvertTo-Json -InputObject $Value -Depth 8|Set-Content -LiteralPath $temporary -Encoding utf8
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}
$server=Join-Path $runtime 'usbbridge-server.exe'
$client=Join-Path $runtime 'usbbridge-client.exe'
$inventory=Join-Path $runtime 'devices.inventory'
$state=@{Backend='Usbipd';DesiredBackend=$Backend;NativePid=$null;NativeStartedUtc=$null;NativeCreatedFileTime=$null;ForcedByUs=@();Reservations=@();PendingStockReleases=@()}
if(Test-Path -LiteralPath $statePath){$previous=Get-Content -LiteralPath $statePath -Raw|ConvertFrom-Json;foreach($property in $previous.PSObject.Properties){$state[$property.Name]=$property.Value}}
$state.DesiredBackend=$Backend
$state.ExportedBusIds=@($configuration.Export)
. (Join-Path $PSScriptRoot 'Write-DebugEvent.ps1')
Write-BridgeEvent $runtime 'backend.begin' @{backend=$Backend;stop=[bool]$Stop;exports=@($configuration.Export)}
$usbipd=Join-Path (Get-ItemProperty -LiteralPath HKLM:\SOFTWARE\usbipd-win -Name APPLICATIONFOLDER).APPLICATIONFOLDER 'usbipd.exe'
function Get-UsbState {
    $text=& $usbipd state|Out-String
    if($LASTEXITCODE-ne 0){throw 'Cannot query physical USB inventory.'}
    ($text|ConvertFrom-Json).Devices
}
function Get-Cached {
    if(-not(Test-Path -LiteralPath $inventory)){return @()}
    $text=& $client inventory $inventory|Out-String
    if($LASTEXITCODE-ne 0){throw 'Cannot read cached device identities.'}
    $values=$text|ConvertFrom-Json
    foreach($value in $values){$value}
}
function Invoke-Usbipd([string[]]$Arguments){
    $operation=Start-Process $usbipd -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $runtime 'backend-usbipd.log') -RedirectStandardError (Join-Path $runtime 'backend-usbipd-errors.log')
    if($operation.ExitCode-ne 0){throw ('usbipd '+($Arguments-join ' ')+' failed: '+$operation.ExitCode)}
}
function Stop-OwnedNative {
    if($state.NativePid){
        $process=Get-Process -Id $state.NativePid -ErrorAction SilentlyContinue
        if($process-and $process.Path-eq $server-and $process.StartTime.ToUniversalTime().ToString('o')-eq $state.NativeStartedUtc){Stop-Process -Id $process.Id -Force;$process.WaitForExit()}
        $state.NativePid=$null;$state.NativeStartedUtc=$null;$state.NativeCreatedFileTime=$null
    }
}
function Repair-StockReleases {
    if(-not @($state.PendingStockReleases).Count){return}
    $cached=@(Get-Cached|Where-Object {$configuration.Export-contains $_.busid})
    $remaining=@()
    foreach($entry in @($state.PendingStockReleases)){
        if(-not @($cached|Where-Object path -eq $entry.InstanceId).Count){continue}
        $problem=(Get-PnpDeviceProperty -InstanceId $entry.StubInstanceId -KeyName DEVPKEY_Device_ProblemCode -ErrorAction SilentlyContinue).Data
        if($problem-eq 21){
            try{& (Join-Path $PSScriptRoot 'Restart-UsbPort.ps1') -DeviceInstanceId $entry.InstanceId -CapturedInstanceId $entry.StubInstanceId -Port ([int]$entry.Port)}
            catch{$remaining+=$entry;Write-JsonFile $statePath $state;throw}
        }
    }
    $state.PendingStockReleases=$remaining
    Write-JsonFile $statePath $state
}
function Stop-Stock {
    if((Get-Service usbipd).Status-ne 'Stopped'){
        $cached=@(Get-Cached|Where-Object {$configuration.Export-contains $_.busid})
        $pending=@($state.PendingStockReleases)
        $captured=@(Get-UsbState|Where-Object {$_.BusId-and $_.StubInstanceId})
        foreach($device in $captured){
            if(@($cached|Where-Object path -eq $device.InstanceId).Count-and -not @($pending|Where-Object InstanceId -eq $device.InstanceId).Count){
                $pending+=[PSCustomObject]@{InstanceId=$device.InstanceId;StubInstanceId=$device.StubInstanceId;Port=[int]($device.BusId.Split('-')[-1])}
            }
        }
        $state.PendingStockReleases=$pending
        Write-JsonFile $statePath $state
        # Let the stock worker finish its normal driver cleanup before the service exits.
        foreach($device in @($captured|Where-Object {$entry=$_;$_.ClientIPAddress-and @($cached|Where-Object path -eq $entry.InstanceId).Count})){
            try{Invoke-Usbipd @('detach','--busid',$device.BusId)}catch{Write-Warning $_.Exception.Message}
        }
        if($pending.Count){
            $deadline=[DateTime]::UtcNow.AddSeconds(5)
            do{
                $physical=@(Get-UsbState)
                $waiting=@($pending|Where-Object {$entry=$_;-not @($physical|Where-Object {$_.InstanceId-eq $entry.InstanceId-and $_.BusId-and -not $_.StubInstanceId}).Count})
                if(-not $waiting.Count){break}
                Start-Sleep -Milliseconds 200
            }while([DateTime]::UtcNow-lt $deadline)
        }
        Stop-Service usbipd
    }
    Repair-StockReleases
}
function Upgrade-Reservations {
    $physical=@(Get-UsbState)
    $cached=@(Get-Cached)
    foreach($busid in @($state.ForcedByUs)){
        if(@($state.Reservations|Where-Object BusId -eq $busid).Count){continue}
        $record=@($cached|Where-Object busid -eq $busid)
        $device=@($physical|Where-Object {($record.Count-eq 1-and $_.InstanceId-eq $record[0].path)-or ($record.Count-eq 0-and $_.BusId-eq $busid)})
        if($device.Count-eq 1){$state.Reservations=@($state.Reservations)+[PSCustomObject]@{BusId=$busid;InstanceId=$device[0].InstanceId;Guid=$device[0].PersistedGuid;WasShared=$true}}
    }
}
function Restore-Stock {
    Stop-OwnedNative
    if((Get-Service usbipd).Status-eq 'Stopped'){Repair-StockReleases}
    Upgrade-Reservations
    $remaining=@();$failures=@()
    foreach($reservation in @($state.Reservations)){
        try{
            $physical=@(Get-UsbState)
            $device=@($physical|Where-Object InstanceId -eq $reservation.InstanceId)
            $guid=if($device.Count-eq 1){$device[0].PersistedGuid}else{$reservation.Guid}
            if($guid){Invoke-Usbipd @('unbind','--guid',$guid)}
            $physical=@(Get-UsbState)
            $device=@($physical|Where-Object { $_.InstanceId-eq $reservation.InstanceId -and $_.BusId })
            if($reservation.WasShared-and $device.Count-eq 1){Invoke-Usbipd @('bind','--busid',$device[0].BusId)}
        }catch{$remaining+=$reservation;$failures+=$_.Exception.Message}
    }
    $state.Reservations=$remaining;$state.ForcedByUs=@($remaining|ForEach-Object BusId)
    $service=Get-CimInstance Win32_Service -Filter "Name='usbipd'"
    if($service.PathName-notmatch ('^"?'+[regex]::Escape($usbipd)+'"?\s+server(?:\s+usbipd:Port=\d+)?\s*$')){throw 'Unexpected usbipd service command; refusing to replace it.'}
    $command='"'+$usbipd+'" server usbipd:Port='+$tcpPort
    if($service.PathName-ne $command){
        Stop-Stock
        $changed=$service|Invoke-CimMethod -MethodName Change -Arguments @{PathName=$command}
        if($changed.ReturnValue-ne 0){throw ('Cannot set stock TCP port: '+$changed.ReturnValue)}
    }
    $stockRule=Get-NetFirewallRule -Name 'USBBridge-Stock' -ErrorAction SilentlyContinue
    if($stockRule){
        if(($stockRule|Get-NetFirewallApplicationFilter).Program-ne $usbipd){throw 'Stock firewall rule belongs to another installation.'}
        $stockRule|Set-NetFirewallRule -RemoteAddress $configuration.AllowedPeers -LocalPort $tcpPort
    }else{New-NetFirewallRule -Name 'USBBridge-Stock' -DisplayName 'USB to Network Bridge: stock server' -Direction Inbound -Action Allow -Protocol TCP -LocalPort $tcpPort -RemoteAddress $configuration.AllowedPeers -Program $usbipd -Profile Any|Out-Null}
    Get-NetFirewallRule -DisplayName 'usbipd' -ErrorAction SilentlyContinue|Set-NetFirewallRule -RemoteAddress $configuration.AllowedPeers -LocalPort $tcpPort
    Start-Service usbipd
    $state.Backend='Usbipd'
    if($failures.Count){throw ($failures-join '; ')}
}
try {
    foreach($file in @($server,$client)){if(-not(Test-Path -LiteralPath $file)){throw ('Missing application: '+$file)}}
    if(@($state.ForcedByUs).Count){Upgrade-Reservations}
    Write-JsonFile $statePath $state
    if($Stop){
        Stop-OwnedNative
        Stop-Stock
        foreach($entry in @($state.Reservations|Where-Object {@($state.LocalWhenIdle)-contains $_.InstanceId})){
            $device=@(Get-UsbState|Where-Object InstanceId -eq $entry.InstanceId)
            $guid=if($device.Count-eq 1-and $device[0].PersistedGuid){$device[0].PersistedGuid}else{$entry.Guid}
            if($guid){Invoke-Usbipd @('unbind','--guid',$guid)}
            $state.Reservations=@($state.Reservations|Where-Object InstanceId -ne $entry.InstanceId)
            $state.ForcedByUs=@($state.ForcedByUs|Where-Object {$_-ne $entry.BusId})
        }
        $state.Backend='Stopped'
    }elseif($Backend-eq 'Usbipd'){Restore-Stock}else{
        Stop-OwnedNative
        $cached=@(Get-Cached)
        $unknown=@($configuration.Export|Where-Object {$selected=$_;-not @($cached|Where-Object busid -eq $selected).Count})
        if((Get-Service usbipd).Status-eq 'Running'){
            if(@($configuration.Export).Count-gt 0-and ($unknown.Count-or $cached.Count-eq 0)){
                $stockService=Get-CimInstance Win32_Service -Filter "Name='usbipd'"
                $stockPort=if($stockService.PathName-match 'usbipd:Port=(\d+)'){[uint16]$Matches[1]}else{3240}
                & $client snapshot ('127.0.0.1:'+$stockPort) $inventory @($configuration.Export)
                if($LASTEXITCODE-ne 0){throw 'Share the selected devices once to capture their identities.'}
                $cached=@(Get-Cached)
            }
            Stop-Stock
        }
        Repair-StockReleases
        $cached=@(Get-Cached)
        foreach($busid in @($configuration.Export)){
            $record=@($cached|Where-Object busid -eq $busid)
            if($record.Count-ne 1){throw ('No cached identity for '+$busid)}
            if($configuration.ExpectedInstanceIds-and $configuration.ExpectedInstanceIds.ContainsKey($busid)-and $configuration.ExpectedInstanceIds[$busid]-ne $record[0].path){throw 'Cached device identity differs from the selected profile.'}
            $physical=@(Get-UsbState)
            $device=@($physical|Where-Object {$_.InstanceId-eq $record[0].path-and $_.BusId})
            if($device.Count-eq 0){Write-Warning ('Waiting for USB device '+$busid);continue}
            if($device.Count-ne 1){throw 'Ambiguous physical device identity.'}
            if(@($state.LocalWhenIdle)-contains $record[0].path){
                $reservation=@($state.Reservations|Where-Object InstanceId -eq $record[0].path)
                if($reservation.Count-and $device[0].IsForced){
                    Invoke-Usbipd @('unbind','--guid',$device[0].PersistedGuid)
                    $state.Reservations=@($state.Reservations|Where-Object InstanceId -ne $record[0].path)
                    $state.ForcedByUs=@($state.ForcedByUs|Where-Object {$_-ne $busid})
                    Write-JsonFile $statePath $state
                }
                continue
            }
            if(-not $device[0].IsForced){
                if(-not @($state.Reservations|Where-Object InstanceId -eq $record[0].path).Count){
                    $state.Reservations=@($state.Reservations)+[PSCustomObject]@{BusId=$busid;InstanceId=$record[0].path;Guid=$device[0].PersistedGuid;WasShared=[bool]$device[0].PersistedGuid}
                }
                $state.ForcedByUs=@(@($state.ForcedByUs)+$busid|Select-Object -Unique)
                Write-JsonFile $statePath $state
                Invoke-Usbipd @('bind','--force','--busid',$device[0].BusId)
                $updated=@(Get-UsbState|Where-Object InstanceId -eq $record[0].path)
                if($updated.Count-eq 1){foreach($entry in @($state.Reservations|Where-Object InstanceId -eq $record[0].path)){$entry.Guid=$updated[0].PersistedGuid}}
                Write-JsonFile $statePath $state
            }
        }
        $rule=Get-NetFirewallRule -Name 'USBBridge-Native' -ErrorAction SilentlyContinue
        if($rule){
            if(($rule|Get-NetFirewallApplicationFilter).Program-ne $server){throw 'USBBridge-Native rule belongs to another installation.'}
            $rule|Set-NetFirewallRule -RemoteAddress $configuration.AllowedPeers -LocalPort $tcpPort
        }else{New-NetFirewallRule -Name 'USBBridge-Native' -DisplayName 'USB to Network Bridge: native server' -Direction Inbound -Action Allow -Protocol TCP -LocalPort $tcpPort -RemoteAddress $configuration.AllowedPeers -Program $server -Profile Any|Out-Null}
        $locations=@(Get-UsbState|Where-Object BusId|ForEach-Object {[ordered]@{identity=$_.InstanceId;busid=$_.BusId}})
        Write-JsonFile (Join-Path $runtime 'device-locations.local.json') $locations
        $publication=Join-Path $runtime 'native-publication.local.json'
        Write-JsonFile $publication ([ordered]@{version=1;revision=[guid]::NewGuid().ToString();devices=@($cached|Where-Object {$configuration.Export-contains $_.busid});local_when_idle=@($state.LocalWhenIdle|Where-Object {$_})})
        $arguments=@('--backend','native','--inventory',('"'+$inventory+'"'),'--publication',('"'+$publication+'"'),'--capture-helper',('"'+(Join-Path $runtime 'Native-Capture.ps1')+'"'),'--listen',$configuration.Listen,'--session-status',('"'+(Join-Path $runtime 'native-sessions.local.json')+'"'))
        if($configuration.Debug){$arguments+='--debug'}
        foreach($busid in @($configuration.Export)){$arguments+=@('--export',$busid)}
        foreach($address in @($configuration.AllowedPeers)){$arguments+=@('--allow-ip',$address)}
        $native=Start-Process -FilePath $server -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $runtime 'native-server.log') -RedirectStandardError (Join-Path $runtime 'native-server-errors.log')
        $state.NativePid=$native.Id;$state.NativeStartedUtc=$native.StartTime.ToUniversalTime().ToString('o')
        $state.NativeCreatedFileTime=$native.StartTime.ToFileTimeUtc().ToString()
        Start-Sleep -Milliseconds 500
        if($native.HasExited){throw 'Native exited at startup; see native-server-errors.log.'}
        $state.Backend='Native'
    }
    Write-JsonFile $statePath $state
    Write-JsonFile $statusPath ([ordered]@{Success=$true;Backend=$state.Backend;NativePid=$state.NativePid;ReconnectRequired=$true})
    Write-BridgeEvent $runtime 'backend.complete' @{backend=$state.Backend;native_pid=$state.NativePid}
}catch{
    $failure=$_.Exception.Message
    Write-BridgeEvent $runtime 'backend.error' @{backend=$Backend;error=$failure}
    if(-not $Stop){try{Restore-Stock}catch{$failure+='; stock rollback: '+$_.Exception.Message}}
    Write-JsonFile $statePath $state
    Write-JsonFile $statusPath ([ordered]@{Success=$false;Error=$failure;Backend=$state.Backend})
    exit 1
}finally{if($owned){$mutex.ReleaseMutex()};$mutex.Dispose()}
