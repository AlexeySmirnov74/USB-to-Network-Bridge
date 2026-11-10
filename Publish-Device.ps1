# Change only the selected USB identity; leave the transport and its other sessions intact.
param([Parameter(Mandatory=$true)][string]$Config,[Parameter(Mandatory=$true)][string]$RequestFile,[switch]$Elevate)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$Config=[IO.Path]::GetFullPath($Config);$RequestFile=[IO.Path]::GetFullPath($RequestFile);$resultFile=$RequestFile+'.result.json'
function Write-Atomic($Path,$Text){$temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp';try{[IO.File]::WriteAllText($temporary,$Text,[Text.UTF8Encoding]::new($true));Move-Item -LiteralPath $temporary -Destination $Path -Force}finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}}
function Write-Json($Path,$Value){Write-Atomic $Path (ConvertTo-Json -InputObject $Value -Depth 12)}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'Administrator required to publish a physical USB device.'}
    try{$child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Config',('"'+$Config+'"'),'-RequestFile',('"'+$RequestFile+'"') -PassThru;$child.WaitForExit();exit $child.ExitCode}
    catch{Write-Json $resultFile ([ordered]@{Success=$false;Error=$_.Exception.Message});exit 1}
}
$owned=$false;$mutex=$null;$changed=$false;$bindingChanged=$false;$temporaryFiles=@();$backups=@{}
try{
    $hostData=Get-Content -LiteralPath $Config -Raw|ConvertFrom-Json
    $runtime=(Resolve-Path -LiteralPath $hostData.runtime).Path;$profile=(Resolve-Path -LiteralPath $hostData.profile).Path
    . (Join-Path $PSScriptRoot 'Write-DebugEvent.ps1')
    $request=Get-Content -LiteralPath $RequestFile -Raw|ConvertFrom-Json
    if($request.publish-isnot [bool]-or $request.identity-notlike 'USB\*'){throw 'Select an explicit USB identity and publication state.'}
    if($request.publish-and $request.identity-like 'USB\VID_1209&PID_F002*'){throw 'Generated USB cameras are managed in Receive Video settings and cannot be captured as physical USB exports.'}
    $forget=$request.PSObject.Properties.Name-contains 'forget'-and $request.forget-eq $true
    if($forget-and $request.publish){throw 'Cannot forget a published device without releasing its publication.'}
    $hasPolicy=$request.PSObject.Properties.Name-contains 'local_when_idle'
    if($hasPolicy-and $request.local_when_idle-isnot [bool]){throw 'Local use must be a Boolean option.'}
    . (Join-Path $PSScriptRoot 'New-BackendMutex.ps1')
    $mutex=New-BridgeBackendMutex $runtime
    try{$owned=$mutex.WaitOne(45000)}catch [Threading.AbandonedMutexException]{$owned=$true}
    if(-not $owned){throw 'Another backend change is still in progress.'}
    $client=Join-Path $runtime 'usbbridge-client.exe';$inventory=Join-Path $runtime 'devices.inventory'
    $publication=Join-Path $runtime 'native-publication.local.json';$statePath=Join-Path $runtime 'backend-state.json'
    $usbipd=Join-Path (Get-ItemProperty -LiteralPath HKLM:\SOFTWARE\usbipd-win -Name APPLICATIONFOLDER).APPLICATIONFOLDER 'usbipd.exe'
    function Get-Physical {$text=& $usbipd state|Out-String;if($LASTEXITCODE-ne 0){throw 'Cannot inspect USB devices.'};@(($text|ConvertFrom-Json).Devices)}
    function Get-Inventory {$text=& $client inventory $inventory|Out-String;if($LASTEXITCODE-ne 0){throw 'Cannot read cached identities.'};@($text|ConvertFrom-Json|ForEach-Object {$_})}
    function Invoke-Usbipd([string[]]$Arguments){
        # PowerShell 5 treats informational native stderr as terminating errors when piped.
        # Judge the operation by its exit code and retain warnings in the development log.
        $token=Join-Path $runtime ('publication-usbipd.'+[guid]::NewGuid().ToString('N'))
        $elapsed=[Diagnostics.Stopwatch]::StartNew()
        Write-BridgeEvent $runtime 'publication.driver.begin' @{identity=$request.identity;arguments=$Arguments}
        try{
            $operation=Start-Process $usbipd -ArgumentList $Arguments -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput ($token+'.stdout.tmp') -RedirectStandardError ($token+'.stderr.tmp')
            $message=[IO.File]::ReadAllText($token+'.stderr.tmp')
            Write-BridgeEvent $runtime 'publication.driver' @{identity=$request.identity;operation=$Arguments[0];arguments=$Arguments;driver_pid=$operation.Id;duration_ms=$elapsed.ElapsedMilliseconds;exit_code=$operation.ExitCode;message=$message}
            if($operation.ExitCode-ne 0){throw ('usbipd '+($Arguments-join ' ')+' failed: '+$message)}
        }finally{foreach($suffix in @('.stdout.tmp','.stderr.tmp')){if(Test-Path -LiteralPath ($token+$suffix)){Remove-Item -LiteralPath ($token+$suffix) -Force}}}
    }
    function Wait-TargetRelease {
        $sessions=Join-Path $runtime 'native-sessions.local.json';$deadline=[DateTime]::UtcNow.AddSeconds(20)
        do{
            if(-not(Test-Path -LiteralPath $sessions)){throw 'Cannot confirm release of the selected device.'}
            $current=Get-Content -LiteralPath $sessions -Raw|ConvertFrom-Json
            if($current.process_id-ne $state.NativePid){throw 'Transport changed during publication.'}
            $idleHeld=$false;$idleStatus=Join-Path $runtime 'native-hid-guards.local.json'
            if(Test-Path -LiteralPath $idleStatus){
                $idle=Get-Content -LiteralPath $idleStatus -Raw|ConvertFrom-Json
                $idleHeld=$idle.process_id-eq $state.NativePid-and ($idle.revision-ne $committedRevision-or @($idle.held|Where-Object {$_-eq $request.identity}).Count-gt 0)
            }
            if(-not $idleHeld-and -not @($current.connections|Where-Object identity -eq $request.identity).Count){return}
            Start-Sleep -Milliseconds 200
        }while([DateTime]::UtcNow-lt $deadline)
        throw 'The selected USB session did not release; other sessions were left running.'
    }
    function Test-TargetCapture {
        $text=& $client capture-state $inventory|Out-String
        if($LASTEXITCODE-ne 0){throw 'Cannot inspect capture interfaces.'}
        return @($text|ConvertFrom-Json|ForEach-Object {$_}|Where-Object path -eq $request.identity).Count-gt 0
    }
    function Restart-TargetPort {
        $current=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
        if($current.Count-ne 1-or -not $current[0].BusId){throw 'Selected device disconnected before its driver could be activated.'}
        $port=[int]($current[0].BusId.Split('-')[1])
        Write-BridgeEvent $runtime 'publication.port.restart' @{identity=$request.identity;busid=$current[0].BusId}
        & (Join-Path $runtime 'Restart-UsbPort.ps1') -DeviceInstanceId $request.identity -Port $port -CapturedInstanceId $current[0].StubInstanceId
        if(-not $?){throw 'Cannot activate the driver of the selected device.'}
    }
    $data=Import-PowerShellDataFile -LiteralPath $profile
    $oldProfile=Get-Content -LiteralPath $profile -Raw
    if([regex]::Matches($oldProfile,'(?im)^\s*Export\s*=\s*@\([^\r\n]*\)').Count-ne 1){throw 'Unsupported Export profile format.'}
    if([regex]::Matches($oldProfile,'(?im)^\s*ExpectedInstanceIds\s*=').Count-gt 1){throw 'Ambiguous identity profile.'}
    $oldState=Get-Content -LiteralPath $statePath -Raw;$previous=$oldState|ConvertFrom-Json;$state=@{}
    foreach($property in $previous.PSObject.Properties){$state[$property.Name]=$property.Value}
    $running=($hostData.PSObject.Properties.Name-notcontains 'enabled')-or [bool]$hostData.enabled
    if($state.DesiredBackend-notin @('Native','Usbipd')-or ($state.Backend-ne $state.DesiredBackend-and ($running-or $state.Backend-ne 'Stopped'))){throw 'Wait for the capture module change to finish.'}
    $native=$state.DesiredBackend-eq 'Native'
    if($native-and $running){
        $process=Get-CimInstance Win32_Process -Filter ('ProcessId='+[int]$state.NativePid)
        if(-not $process-or $process.ExecutablePath-ne (Join-Path $runtime 'usbbridge-server.exe')-or $process.CommandLine-notmatch '(?:^|\s)--publication(?:\s|$)'){throw 'Update the running Native transport to support live publication first.'}
        if(-not(Test-Path -LiteralPath $publication)){throw 'Live publication snapshot is missing.'}
    }
    $cached=@(Get-Inventory);$selected=@($cached|Where-Object {$data.Export-contains $_.busid})
    if($selected.Count-ne @($data.Export).Count){throw 'A profile identity is missing from the descriptor inventory.'}
    $physical=@(Get-Physical);$device=@($physical|Where-Object InstanceId -eq $request.identity)
    if($device.Count-gt 1){throw 'Ambiguous physical USB identity.'}
    if($forget-and @($device|Where-Object BusId).Count){throw 'The device is connected to USB. Refresh the device list.'}
    $already=@($selected|Where-Object path -eq $request.identity).Count-gt 0
    $localPolicies=@($state.LocalWhenIdle|Where-Object {$_})
    $localBefore=$localPolicies-contains $request.identity
    $localAfter=if($hasPolicy){[bool]$request.local_when_idle}else{$localBefore}
    if($forget){$localAfter=$false}
    $policyChanged=$localAfter-ne $localBefore
    if($policyChanged-and $native-and $running-and $process.CommandLine-notmatch '(?:^|\s)--capture-helper(?:\s|$)'){throw 'Update the Native transport before changing local-use options.'}
    $idempotent=$already-eq $request.publish-and -not $policyChanged-and -not $forget
    if(-not $native-and $device.Count-eq 1-and [bool]$device[0].PersistedGuid-ne $request.publish){$idempotent=$false}
    if($idempotent){Write-Json $resultFile ([ordered]@{Success=$true;Published=[bool]$request.publish;ReconnectRequired=$false;Changed=$false});exit 0}
    if($request.publish-and -not $already-and ($device.Count-ne 1-or -not $device[0].BusId)){throw 'The selected USB device is no longer present. Refresh the list.'}
    $wasShared=$device.Count-eq 1-and [bool]$device[0].PersistedGuid;$wasForced=$device.Count-eq 1-and [bool]$device[0].IsForced
    $oldGuid=if($wasShared){$device[0].PersistedGuid}else{$null}
    if(-not $oldGuid){$reservation=@($state.Reservations|Where-Object InstanceId -eq $request.identity);if($reservation.Count-eq 1){$oldGuid=$reservation[0].Guid}}
    Write-BridgeEvent $runtime 'publication.begin' @{identity=$request.identity;publish=$request.publish;local_when_idle=$localAfter;policy_changed=$policyChanged;backend=$state.Backend;mode='live';native_pid=$state.NativePid}
    foreach($path in @($profile,$statePath,$inventory,$publication)){
        if(Test-Path -LiteralPath $path){$copy=$path+'.'+[guid]::NewGuid().ToString('N')+'.tmp';Copy-Item -LiteralPath $path -Destination $copy;$backups[$path]=$copy;$temporaryFiles+=$copy}
    }
    $changed=$true;$identities=@($selected|ForEach-Object path)
    $state.LocalWhenIdle=@($localPolicies|Where-Object {$_-ne $request.identity})
    if($localAfter){$state.LocalWhenIdle+=@($request.identity)}
    if($request.publish-and -not $already){
        $identities+=@($request.identity)
        $query=$inventory+'.'+[guid]::NewGuid().ToString('N')+'.request.tmp';$snapshot=$query+'.inventory';$temporaryFiles+=@($query,$snapshot)
        $node=if($device[0].StubInstanceId){$device[0].StubInstanceId}else{$device[0].InstanceId}
        Write-Json $query ([ordered]@{identity=$request.identity;node_instance=$node;busid=$device[0].BusId})
        & $client snapshot-local $query $snapshot
        if($LASTEXITCODE-ne 0){throw 'Cannot read descriptors of the selected device through its parent USB hub.'}
        & $client merge-inventory $inventory $snapshot
        if($LASTEXITCODE-ne 0){throw 'Cannot update USB descriptor inventory.'}
    }elseif(-not $request.publish){$identities=@($identities|Where-Object {$_-ne $request.identity})}
    $cached=@(Get-Inventory);$selected=@($cached|Where-Object {$identities-contains $_.path})
    if($selected.Count-ne $identities.Count){throw 'A selected identity is missing from the descriptor inventory.'}
    $fresh=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
    if($forget-and @($fresh|Where-Object BusId).Count){throw 'The device returned to USB. Refresh the device list.'}
    if($request.publish){
        if(-not $already-and ($fresh.Count-ne 1-or $fresh[0].BusId-ne $device[0].BusId)){throw 'The selected USB device moved during publication; refresh and retry.'}
        $record=@($selected|Where-Object path -eq $request.identity)[0]
        if($native-and $running-and -not $localAfter-and $fresh.Count-eq 1-and -not $fresh[0].IsForced){
            if(-not @($state.Reservations|Where-Object InstanceId -eq $request.identity).Count){$state.Reservations=@($state.Reservations)+[PSCustomObject]@{BusId=$record.busid;InstanceId=$request.identity;Guid=$fresh[0].PersistedGuid;WasShared=[bool]$fresh[0].PersistedGuid}}
            $state.ForcedByUs=@(@($state.ForcedByUs)+$record.busid|Select-Object -Unique)
            Write-Json $statePath $state;$bindingChanged=$true
            Invoke-Usbipd @('bind','--force','--busid',$fresh[0].BusId)
            $bound=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
            if($bound.Count-ne 1-or -not $bound[0].IsForced){throw 'The selected device was not reserved.'}
            foreach($entry in @($state.Reservations|Where-Object InstanceId -eq $request.identity)){$entry.Guid=$bound[0].PersistedGuid}
            if(-not(Test-TargetCapture)){
                # Some composite devices retain their old driver until their own port is cycled.
                # Never restart a controller or touch a different port to activate this binding.
                Restart-TargetPort
                $deadline=[DateTime]::UtcNow.AddSeconds(10)
                do{Start-Sleep -Milliseconds 300;$ready=Test-TargetCapture}while(-not $ready-and [DateTime]::UtcNow-lt $deadline)
                if(-not $ready){throw 'The selected capture interface did not become available.'}
            }
        }elseif(-not $native-and $fresh.Count-eq 1-and -not $fresh[0].PersistedGuid){$bindingChanged=$true;Invoke-Usbipd @('bind','--busid',$fresh[0].BusId)}
    }
    $exports='@('+((@($selected|ForEach-Object {"'"+$_.busid+"'"}))-join ',')+')'
    $expected='@{'+((@($selected|ForEach-Object {"'"+$_.busid+"' = '"+$_.path.Replace("'","''")+"'"}))-join '; ')+'}'
    $newProfile=[regex]::Replace($oldProfile,'(?im)^(\s*Export\s*=\s*)@\([^\r\n]*\)',[Text.RegularExpressions.MatchEvaluator]{param($match) $match.Groups[1].Value+$exports})
    if($newProfile-match '(?im)^\s*ExpectedInstanceIds\s*=\s*@\{'){$newProfile=[regex]::Replace($newProfile,'(?ims)^(\s*ExpectedInstanceIds\s*=\s*)@\{.*?\}',[Text.RegularExpressions.MatchEvaluator]{param($match) $match.Groups[1].Value+$expected})}
    else{$newProfile=[regex]::Replace($newProfile,'(?m)^@\{\s*$',[Text.RegularExpressions.MatchEvaluator]{param($match) '@{'+[Environment]::NewLine+'    ExpectedInstanceIds = '+$expected})}
    Write-Atomic $profile $newProfile
    $state.ExportedBusIds=@($selected|ForEach-Object busid);Write-Json $statePath $state
    # The commit is one atomic replacement. Readers retain the last valid snapshot on read errors.
    $committedRevision=[guid]::NewGuid().ToString()
    Write-Json $publication ([ordered]@{version=1;revision=$committedRevision;devices=@($selected);local_when_idle=@($state.LocalWhenIdle)})
    Write-BridgeEvent $runtime 'publication.committed' @{identity=$request.identity;publish=$request.publish;native_pid=$state.NativePid}
    if(-not $request.publish){
        if($native-and $running){Wait-TargetRelease}
        $target=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
        if($forget-and @($target|Where-Object BusId).Count){throw 'The device returned to USB. Refresh the device list.'}
        $guid=if($target.Count-eq 1-and $target[0].PersistedGuid){$target[0].PersistedGuid}elseif(-not $forget){$oldGuid}else{$null}
        if($guid){$bindingChanged=$true;Write-BridgeEvent $runtime 'publication.release' @{identity=$request.identity;guid=$guid};Invoke-Usbipd @('unbind','--guid',$guid)}
        if(-not $forget-and $native-and (Test-TargetCapture)){Restart-TargetPort}
        $removedSlots=@($cached|Where-Object path -eq $request.identity|ForEach-Object busid)
        $state.Reservations=@($state.Reservations|Where-Object InstanceId -ne $request.identity)
        $state.ForcedByUs=@($state.ForcedByUs|Where-Object {$removedSlots-notcontains $_})
        Write-Json $statePath $state
    }
    if($forget){
        # A final physical-presence check, inventory pruning and catalog tombstone share a file lock.
        # No Windows devnode is uninstalled and no USB port/controller is restarted here.
        & $client forget-device-cache $Config $request.identity
        if($LASTEXITCODE-ne 0){throw 'Cannot forget the USB device; it may have reconnected. Refresh the list.'}
        Write-BridgeEvent $runtime 'device.forgotten' @{identity=$request.identity;native_pid=$state.NativePid}
    }
    Write-BridgeEvent $runtime 'publication.complete' @{identity=$request.identity;publish=$request.publish;exports=@($selected|ForEach-Object path);mode='live';native_pid=$state.NativePid}
    Write-Json $resultFile ([ordered]@{Success=$true;Published=[bool]$request.publish;ReconnectRequired=$false;Changed=$true})
}catch{
    $failure=$_.Exception.Message
    if($changed){
        try{
            if($bindingChanged){
                $target=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
                if($target.Count-eq 1){
                    if($target[0].PersistedGuid-and (-not $wasShared-or [bool]$target[0].IsForced-ne $wasForced)){Invoke-Usbipd @('unbind','--guid',$target[0].PersistedGuid)}
                    $target=@(Get-Physical|Where-Object InstanceId -eq $request.identity)
                    if($wasShared-and $target.Count-eq 1-and $target[0].BusId){if($wasForced){Invoke-Usbipd @('bind','--force','--busid',$target[0].BusId)}else{Invoke-Usbipd @('bind','--busid',$target[0].BusId)}}
                }
            }
        }catch{$failure+='; selected-device rollback: '+$_.Exception.Message}
        foreach($path in @($inventory,$profile,$statePath,$publication)){
            try{if($backups.ContainsKey($path)){Move-Item -LiteralPath $backups[$path] -Destination $path -Force}elseif($path-eq $publication-and (Test-Path -LiteralPath $path)){Remove-Item -LiteralPath $path -Force}}
            catch{$failure+='; restore '+[IO.Path]::GetFileName($path)+': '+$_.Exception.Message}
        }
    }
    if($runtime){Write-BridgeEvent $runtime 'publication.error' @{identity=$request.identity;error=$failure;mode='live'}}
    Write-Json $resultFile ([ordered]@{Success=$false;Error=$failure});exit 1
}finally{
    foreach($temporary in $temporaryFiles){if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
    if($owned){$mutex.ReleaseMutex()};if($mutex){$mutex.Dispose()}
}
