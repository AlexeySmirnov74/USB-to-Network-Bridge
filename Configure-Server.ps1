# Apply desktop settings through UAC. Never changes boot security or driver signing.
param([Parameter(Mandatory=$true)][string]$Config,[Parameter(Mandatory=$true)][string]$RequestFile,[switch]$Elevate)
$ErrorActionPreference='Stop'
$Config=[IO.Path]::GetFullPath($Config)
$RequestFile=[IO.Path]::GetFullPath($RequestFile)
$resultFile=$RequestFile+'.result.json'
function Write-Atomic($Path,$Text){
    $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
    [IO.File]::WriteAllText($temporary,$Text,[Text.UTF8Encoding]::new($true))
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'Administrator required to apply server ports and firewall rules.'}
    try{
        $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Config',('"'+$Config+'"'),'-RequestFile',('"'+$RequestFile+'"') -PassThru
        $child.WaitForExit();exit $child.ExitCode
    }catch{Write-Atomic $resultFile (([ordered]@{Success=$false;Error=$_.Exception.Message}|ConvertTo-Json));exit 1}
}
$oldProfile=$null;$oldHost=$null;$configurationChanged=$false
try{
    $hostData=Get-Content -LiteralPath $Config -Raw|ConvertFrom-Json
    $keepStopped=($hostData.PSObject.Properties.Name-contains 'enabled')-and -not [bool]$hostData.enabled
    $runtime=(Resolve-Path -LiteralPath $hostData.runtime).Path
    $profile=(Resolve-Path -LiteralPath $hostData.profile).Path
    $request=Get-Content -LiteralPath $RequestFile -Raw|ConvertFrom-Json
    $autoRemove=if($hostData.PSObject.Properties.Name-contains 'auto_remove_unplugged'){[bool]$hostData.auto_remove_unplugged}else{$true}
    $days=if($hostData.PSObject.Properties.Name-contains 'unplugged_retention_days'){[uint32]$hostData.unplugged_retention_days}else{1}
    if($request.PSObject.Properties.Name-contains 'auto_remove_unplugged'){
        if($request.auto_remove_unplugged-isnot [bool]){throw 'Automatic cleanup must be a Boolean option.'}
        $autoRemove=[bool]$request.auto_remove_unplugged
    }
    if($request.PSObject.Properties.Name-contains 'unplugged_retention_days'){
        if([string]$request.unplugged_retention_days-notmatch '^\d{1,5}$'){throw 'Days must be in 1..36500.'}
        $days=[uint32]$request.unplugged_retention_days
    }
    if($days-lt 1-or $days-gt 36500){throw 'Days must be in 1..36500.'}
    $tcp=[uint16]$request.tcp_port;$udp=[uint16]$request.discovery_port
    if($tcp-eq 0-or $udp-eq 0){throw 'Ports must be in 1..65535.'}
    if($tcp-eq $udp){throw 'USB data and status/discovery ports must differ.'}
    if(-not $request.name-or $request.name.Length-gt 128){throw 'Server name must contain 1..128 characters.'}
    if($request.backend-notin @('Native','Usbipd')){throw 'Invalid capture module.'}
    $peers=@($request.allowed_ips|ForEach-Object {[Net.IPAddress]::Parse($_).ToString()}|Select-Object -Unique)
    if($peers.Count-eq 0){throw 'At least one allowed client IP is required.'}
    $oldProfile=Get-Content -LiteralPath $profile -Raw
    $oldHost=Get-Content -LiteralPath $Config -Raw
    $data=Import-PowerShellDataFile -LiteralPath $profile
    $backendState=Get-Content -LiteralPath (Join-Path $runtime 'backend-state.json') -Raw|ConvertFrom-Json
    # Cleanup policy changes are live: do not stop the controller, transport or active USB sessions.
    $metadataOnly=$hostData.name-eq $request.name-and $hostData.discovery_port-eq $udp-and
        [int]([string]$data.Listen).Substring(([string]$data.Listen).LastIndexOf(':')+1)-eq $tcp-and
        $backendState.DesiredBackend-eq $request.backend-and
        @((Compare-Object @($data.AllowedPeers|Sort-Object -Unique) @($peers|Sort-Object -Unique))).Count-eq 0
    $hostData|Add-Member -NotePropertyName auto_remove_unplugged -NotePropertyValue $autoRemove -Force
    $hostData|Add-Member -NotePropertyName unplugged_retention_days -NotePropertyValue $days -Force
    if($metadataOnly){
        Write-Atomic $Config ($hostData|ConvertTo-Json -Depth 8)
        Write-Atomic $resultFile (([ordered]@{Success=$true;Restarted=$false;AutoRemoveUnplugged=$autoRemove;UnpluggedRetentionDays=$days}|ConvertTo-Json))
        exit 0
    }
    if($data.Listen-notmatch '^(\[[0-9A-Fa-f:]+\]|[0-9.]+):\d+$'){throw 'Invalid server listen address.'}
    $listen=$Matches[1]+':'+$tcp
    $listenPattern="(?im)^(\s*Listen\s*=\s*)'[^']+'"
    $peersPattern='(?im)^(\s*AllowedPeers\s*=\s*)@\([^\r\n]*\)'
    if([regex]::Matches($oldProfile,$listenPattern).Count-ne 1-or [regex]::Matches($oldProfile,$peersPattern).Count-ne 1){throw 'Profile format is not supported by the settings editor. Update it manually.'}
    $newProfile=[regex]::Replace($oldProfile,$listenPattern,('$1'+"'"+$listen+"'"))
    $newProfile=[regex]::Replace($newProfile,$peersPattern,('$1'+'@('+"'"+($peers-join "','")+"'"+')'))
    if($newProfile-match '(?im)^\s*DiscoveryPort\s*='){$newProfile=[regex]::Replace($newProfile,'(?im)^(\s*DiscoveryPort\s*=\s*)\d+',('${1}'+$udp))}
    else{$newProfile=[regex]::Replace($newProfile,'(?m)^@\{\s*$',('@{'+[Environment]::NewLine+'    DiscoveryPort = '+$udp))}
    $agent=Join-Path $runtime 'usbbridge-agent.exe'
    $task=Get-ScheduledTask -TaskPath '\USBBridge\' -TaskName 'USBBridge.Server' -ErrorAction SilentlyContinue
    if($task-and @($task.Actions|Where-Object {$_.Execute-ne $agent-or $_.Arguments-notlike ('*"'+$Config+'"*')}).Count){throw 'Server task belongs to another installation.'}
    if($task){$task|Disable-ScheduledTask|Out-Null;$task|Stop-ScheduledTask}
    foreach($process in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe'"|Where-Object {$_.ExecutablePath-eq $agent-and $_.CommandLine-match '(?:^|\s)--host(?:\s|$)'-and $_.CommandLine.Contains($Config)})){Stop-Process -Id $process.ProcessId -Force}
    $oldBackend=(Get-Content -LiteralPath (Join-Path $runtime 'backend-state.json') -Raw|ConvertFrom-Json).DesiredBackend
    $configurationChanged=$true
    Write-Atomic $profile $newProfile
    $hostData.name=$request.name
    $hostData.discovery_port=$udp
    Write-Atomic $Config ($hostData|ConvertTo-Json -Depth 8)
    & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $request.backend -RuntimeDirectory $runtime -Profile $profile -Stop:$keepStopped
    if(-not $?){throw 'Cannot apply the capture module/port.'}
    & (Join-Path $runtime 'Set-Autostart.ps1') -Role Server -RuntimeDirectory $runtime -Config $Config -Profile $profile
    if(-not $?){throw 'Cannot restart the server recovery task.'}
    Write-Atomic $resultFile (([ordered]@{Success=$true;TcpPort=$tcp;DiscoveryPort=$udp}|ConvertTo-Json))
}catch{
    $failure=$_.Exception.Message
    if($configurationChanged){
        try{
            Write-Atomic $profile $oldProfile;Write-Atomic $Config $oldHost
            if($oldBackend-notin @('Native','Usbipd')){$oldBackend='Native'}
            & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $oldBackend -RuntimeDirectory $runtime -Profile $profile -Stop:$keepStopped
            & (Join-Path $runtime 'Set-Autostart.ps1') -Role Server -RuntimeDirectory $runtime -Config $Config -Profile $profile
        }catch{$failure+='; rollback: '+$_.Exception.Message}
    }
    Write-Atomic $resultFile (([ordered]@{Success=$false;Error=$failure}|ConvertTo-Json));exit 1
}
