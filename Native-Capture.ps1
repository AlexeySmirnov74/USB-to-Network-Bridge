# Own only one selected device for one Native session; all driver changes use
# the same machine-wide mutex as publication, settings and backend recovery.
param([Parameter(Mandatory=$true)][string]$RuntimeDirectory,[Parameter(Mandatory=$true)][string]$RequestFile,[switch]$TryLock)
$ErrorActionPreference='Stop'
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path
$request=Get-Content -LiteralPath $RequestFile -Raw|ConvertFrom-Json
$resultFile=$RequestFile+'.result.json';$owned=$false;$mutex=$null
function Write-Json($Path,$Value){$tmp=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp';try{[IO.File]::WriteAllText($tmp,(ConvertTo-Json -InputObject $Value -Depth 12),[Text.UTF8Encoding]::new($true));Move-Item -LiteralPath $tmp -Destination $Path -Force}finally{if(Test-Path -LiteralPath $tmp){Remove-Item -LiteralPath $tmp -Force}}}
try{
    . (Join-Path $PSScriptRoot 'Write-DebugEvent.ps1')
    . (Join-Path $PSScriptRoot 'New-BackendMutex.ps1')
    if($request.operation-notin @('acquire','release')-or $request.identity-notlike 'USB\*'){throw 'Invalid capture request.'}
    if($request.token){[void][guid]::Parse($request.token)}
    $mutex=New-BridgeBackendMutex $runtime
    try{$owned=$mutex.WaitOne($(if($TryLock){0}else{20000}))}catch [Threading.AbandonedMutexException]{$owned=$true}
    if(-not $owned){Write-Json $resultFile @{Success=$true;Busy=$true};exit 0}
    $statePath=Join-Path $runtime 'backend-state.json';$previous=Get-Content $statePath -Raw|ConvertFrom-Json;$state=@{}
    foreach($property in $previous.PSObject.Properties){$state[$property.Name]=$property.Value}
    if($state.Backend-ne 'Native'-or $state.NativePid-ne $request.process_id){throw 'The Native owner changed.'}
    $owner=Get-Process -Id $state.NativePid -ErrorAction Stop
    if($owner.Path-ne (Join-Path $runtime 'usbbridge-server.exe')-or $owner.StartTime.ToFileTimeUtc().ToString()-ne $state.NativeCreatedFileTime){throw 'The Native process identity changed.'}
    $publication=Get-Content (Join-Path $runtime 'native-publication.local.json') -Raw|ConvertFrom-Json
    $selected=@($publication.devices|Where-Object path -eq $request.identity)
    $local=@($publication.local_when_idle)-contains $request.identity
    $usbipd=Join-Path (Get-ItemProperty -LiteralPath HKLM:\SOFTWARE\usbipd-win -Name APPLICATIONFOLDER).APPLICATIONFOLDER 'usbipd.exe'
    function Physical {$text=& $usbipd state|Out-String;if($LASTEXITCODE-ne 0){throw 'Cannot inspect physical devices.'};@(($text|ConvertFrom-Json).Devices|Where-Object InstanceId -eq $request.identity)}
    function Driver([string[]]$Arguments){
        $output=Join-Path $runtime ('capture-usbipd.'+[guid]::NewGuid().ToString('N'));$timer=[Diagnostics.Stopwatch]::StartNew()
        Write-BridgeEvent $runtime 'capture.driver.begin' @{identity=$request.identity;arguments=$Arguments;capture_token=$request.token}
        try{
            $p=Start-Process $usbipd -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput ($output+'.out.tmp') -RedirectStandardError ($output+'.err.tmp')
            $message=[IO.File]::ReadAllText($output+'.err.tmp')
            Write-BridgeEvent $runtime 'capture.driver.complete' @{identity=$request.identity;arguments=$Arguments;capture_token=$request.token;driver_pid=$p.Id;duration_ms=$timer.ElapsedMilliseconds;exit_code=$p.ExitCode;message=$message}
            if($p.ExitCode-ne 0){throw ('Device driver operation failed: '+$message)}
        }finally{foreach($suffix in @('.out.tmp','.err.tmp')){if(Test-Path -LiteralPath ($output+$suffix)){Remove-Item -LiteralPath ($output+$suffix) -Force}}}
    }
    $entries=@($state.Reservations|Where-Object InstanceId -eq $request.identity)
    if($entries.Count-gt 1){throw 'Ambiguous device reservation.'}
    if($request.operation-eq 'acquire'){
        if($selected.Count-ne 1-or -not $local-or -not $request.token){throw 'Device is not published for on-demand capture.'}
        $physical=@(Physical);if($physical.Count-ne 1-or -not $physical[0].BusId){throw 'The selected physical device is absent.'}
        if(-not $entries.Count){
            $state.Reservations=@($state.Reservations)+[PSCustomObject]@{BusId=$selected[0].busid;InstanceId=$request.identity;Guid=$physical[0].PersistedGuid;WasShared=[bool]$physical[0].PersistedGuid;CaptureToken=$request.token}
        }else{$entries[0]|Add-Member -NotePropertyName CaptureToken -NotePropertyValue $request.token -Force}
        $state.ForcedByUs=@(@($state.ForcedByUs)+$selected[0].busid|Select-Object -Unique)
        Write-Json $statePath $state
        if(-not $physical[0].IsForced){Driver @('bind','--force','--busid',$physical[0].BusId)}
        $physical=@(Physical);if($physical.Count-ne 1-or -not $physical[0].IsForced){throw 'Capture driver was not activated.'}
        foreach($entry in @($state.Reservations|Where-Object InstanceId -eq $request.identity)){$entry.Guid=$physical[0].PersistedGuid}
        Write-Json $statePath $state
        # Probe interfaces without opening or claiming an unrelated USB device.
        $client=Join-Path $runtime 'usbbridge-client.exe';$inventory=Join-Path $runtime 'devices.inventory'
        $text=& $client capture-state $inventory|Out-String
        if($LASTEXITCODE-ne 0){throw 'Cannot inspect capture interfaces.'}
        if(-not @($text|ConvertFrom-Json|ForEach-Object {$_}|Where-Object path -eq $request.identity).Count){
            & (Join-Path $runtime 'Restart-UsbPort.ps1') -DeviceInstanceId $request.identity -Port ([int]$physical[0].BusId.Split('-')[-1]) -CapturedInstanceId $physical[0].StubInstanceId
            if(-not $?){throw 'Cannot activate the selected capture port.'}
            $deadline=[DateTime]::UtcNow.AddSeconds(10)
            do{
                Start-Sleep -Milliseconds 200
                $text=& $client capture-state $inventory|Out-String
                if($LASTEXITCODE-ne 0){throw 'Cannot confirm the selected capture interface.'}
                $ready=@($text|ConvertFrom-Json|ForEach-Object {$_}|Where-Object path -eq $request.identity).Count-gt 0
            }while(-not $ready-and [DateTime]::UtcNow-lt $deadline)
            if(-not $ready){throw 'The selected capture interface did not become ready.'}
        }
        Write-BridgeEvent $runtime 'capture.acquired' @{identity=$request.identity;capture_token=$request.token;native_pid=$request.process_id}
    }else{
        if(-not $entries.Count){Write-Json $resultFile @{Success=$true;Busy=$false};exit 0}
        if($request.token-and $entries[0].CaptureToken-ne $request.token){Write-Json $resultFile @{Success=$true;Busy=$false};exit 0}
        # The user may have disabled local use during an active import. Retain
        # the binding in that case; the normal idle capture takes over instead.
        if($selected.Count-eq 1-and -not $local){
            $entries[0].PSObject.Properties.Remove('CaptureToken');Write-Json $statePath $state
            Write-Json $resultFile @{Success=$true;Busy=$false};exit 0
        }
        $sessions=Get-Content (Join-Path $runtime 'native-sessions.local.json') -Raw|ConvertFrom-Json
        if($sessions.process_id-eq $request.process_id-and @($sessions.connections|Where-Object identity -eq $request.identity).Count){Write-Json $resultFile @{Success=$true;Busy=$true};exit 0}
        $physical=@(Physical);$guid=if($physical.Count-eq 1-and $physical[0].PersistedGuid){$physical[0].PersistedGuid}else{$entries[0].Guid}
        if($guid){Driver @('unbind','--guid',$guid)}
        $physical=@(Physical)
        if($physical.Count-eq 1-and $physical[0].BusId){
            $text=& (Join-Path $runtime 'usbbridge-client.exe') capture-state (Join-Path $runtime 'devices.inventory')|Out-String
            if($LASTEXITCODE-ne 0){throw 'Cannot confirm capture release.'}
            if(@($text|ConvertFrom-Json|ForEach-Object {$_}|Where-Object path -eq $request.identity).Count){
                & (Join-Path $runtime 'Restart-UsbPort.ps1') -DeviceInstanceId $request.identity -Port ([int]$physical[0].BusId.Split('-')[-1]) -CapturedInstanceId $physical[0].StubInstanceId
                if(-not $?){throw 'Cannot restore the selected local driver.'}
            }
        }
        $state.ForcedByUs=@($state.ForcedByUs|Where-Object {$_-ne $entries[0].BusId})
        $state.Reservations=@($state.Reservations|Where-Object InstanceId -ne $request.identity)
        Write-Json $statePath $state
        Write-BridgeEvent $runtime 'capture.local.restored' @{identity=$request.identity;capture_token=$request.token;native_pid=$request.process_id}
    }
    Write-Json $resultFile @{Success=$true;Busy=$false}
}catch{
    if($runtime){Write-BridgeEvent $runtime 'capture.error' @{identity=$request.identity;operation=$request.operation;error=$_.Exception.Message}}
    Write-Json $resultFile @{Success=$false;Busy=$false;Error=$_.Exception.Message};exit 1
}finally{if($owned){$mutex.ReleaseMutex()};if($mutex){$mutex.Dispose()}}
