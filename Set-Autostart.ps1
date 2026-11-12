param(
    [Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,
    [string]$RuntimeDirectory, [string]$Config, [string]$Profile,
    [switch]$Disable, [switch]$Elevate, [switch]$NoTray
)
. (Join-Path $PSScriptRoot 'Portable-Paths.ps1')
$ErrorActionPreference='Stop'
if(-not $RuntimeDirectory){$RuntimeDirectory=if(Test-Path (Join-Path $PSScriptRoot 'usbbridge-agent.exe')){$PSScriptRoot}else{Join-Path $PSScriptRoot 'production'}}
$runtime=(Resolve-Path -LiteralPath $RuntimeDirectory).Path
if(-not $Config){$Config=Join-Path $runtime (($Role.ToLowerInvariant())+'.local.json')}
$Config=[IO.Path]::GetFullPath($Config)
if(-not $Profile){$Profile=Join-Path $runtime 'server.local.psd1'}
$statusPath=Join-Path $runtime ('autostart-'+$Role.ToLowerInvariant()+'.local.json')
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'Run Set-Autostart.ps1 as Administrator, or pass -Elevate for UAC.'}
    $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Role',$Role,'-RuntimeDirectory',('"'+$runtime+'"'),'-Config',('"'+$Config+'"'),'-Profile',('"'+$Profile+'"'))
    if($Disable){$arguments+='-Disable'};if($NoTray){$arguments+='-NoTray'}
    $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
    $child.WaitForExit()
    if(Test-Path -LiteralPath $statusPath){Get-Content -LiteralPath $statusPath -Raw}
    if($child.ExitCode-ne 0){throw 'Autostart configuration failed; see its status file.'}
    exit 0
}
$agent=Join-Path $runtime 'usbbridge-agent.exe'
if(-not(Test-Path -LiteralPath $agent)){throw 'Copy the release application files into the runtime directory first.'}
$launchRoot=if((Split-Path -Leaf $runtime)-eq 'app'){Split-Path -Parent $runtime}else{$runtime}
$launcher=Join-Path $launchRoot ('USB-to-Network-Bridge-'+$Role+'.exe')
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
$taskPath='\USBBridge\'
$taskName='USBBridge.'+$Role
$healthName='USBBridge.Health.'+$Role
$trayName='USBBridge.Tray.'+$Role+'.'+$identity.User.Value
$shortcut=Join-Path ([Environment]::GetFolderPath('Programs')) ('USB to Network Bridge '+$Role+'.lnk')
$legacyShortcut=Join-Path ([Environment]::GetFolderPath('Programs')) ('USBBridge '+$Role+'.lnk')
function Assert-OwnedTask($Name){
    $task=Get-ScheduledTask -TaskPath $taskPath -TaskName $Name -ErrorAction SilentlyContinue
    if($task-and @($task.Actions|Where-Object {$_.Execute-ne $agent-or $_.Arguments-notlike ('*"'+$Config+'"*')}).Count){throw ('Task belongs to another installation: '+$Name)}
    $task
}
function Stop-OwnedAgent($Mode){
    foreach($process in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe'")){
        $selected='(?:^|\s)--config\s+(?:"'+[regex]::Escape($Config)+'"|'+[regex]::Escape($Config)+'(?:\s|$))'
        if($process.ExecutablePath-eq $agent-and $process.CommandLine-match $selected-and $process.CommandLine-match ([regex]::Escape($Mode)+'(?:\s|$)')){
            Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }
}
try {
    $existing=Assert-OwnedTask $taskName
    $existingTray=Assert-OwnedTask $trayName
    $existingHealth=Assert-OwnedTask $healthName
    if($Disable){
        foreach($name in @($healthName,$taskName,$trayName)){
            if(Get-ScheduledTask -TaskPath $taskPath -TaskName $name -ErrorAction SilentlyContinue){Stop-ScheduledTask -TaskPath $taskPath -TaskName $name -ErrorAction SilentlyContinue;Unregister-ScheduledTask -TaskPath $taskPath -TaskName $name -Confirm:$false}
        }
        foreach($linkPath in @($shortcut,$legacyShortcut)){
            if(Test-Path -LiteralPath $linkPath){
                $shell=New-Object -ComObject WScript.Shell
                if($shell.CreateShortcut($linkPath).TargetPath-in @($agent,$launcher)){Remove-Item -LiteralPath $linkPath -Force}
            }
        }
        [ordered]@{Success=$true;Enabled=$false;Role=$Role;Config=$Config}|ConvertTo-Json|Set-Content -LiteralPath $statusPath -Encoding utf8
        exit 0
    }
    if($Role-eq 'Server'){
        $Profile=(Resolve-Path -LiteralPath $Profile).Path
        $profileData=Import-PowerShellDataFile -LiteralPath $Profile
        if($profileData.Listen-notmatch '^(\[[0-9A-Fa-f:]+\]|[0-9.]+):([0-9]{1,5})$'){throw 'Invalid server listen address.'}
        $tcpPort=[uint16]::Parse($Matches[2]);if($tcpPort-eq 0){throw 'TCP port must be in 1..65535.'}
        $probe=if($Matches[1]-eq '0.0.0.0'){'127.0.0.1:'+$tcpPort}elseif($Matches[1]-eq '[::]'){'[::1]:'+$tcpPort}else{$profileData.Listen}
        if(@($profileData.AllowedPeers).Count-eq 0){throw 'The server profile needs explicit AllowedPeers.'}
        foreach($ip in @($profileData.AllowedPeers)){[Net.IPAddress]::Parse($ip)|Out-Null}
        $id=[guid]::NewGuid().ToString()
        $serverName=$env:COMPUTERNAME
        $serverEnabled=$true
        $old=$null
        if(Test-Path -LiteralPath $Config){$old=Get-Content -LiteralPath $Config -Raw|ConvertFrom-Json;[guid]::Parse($old.server_id)|Out-Null;$id=$old.server_id;if($old.name){$serverName=$old.name}}
        if($old-and $old.PSObject.Properties.Name-contains 'enabled'){$serverEnabled=[bool]$old.enabled}
        $discoveryPort=if($profileData.DiscoveryPort){[uint16]$profileData.DiscoveryPort}elseif($old.discovery_port){[uint16]$old.discovery_port}else{3241}
        if($discoveryPort-eq 0){throw 'Discovery port must be in 1..65535.'}
        if($discoveryPort-eq $tcpPort){throw 'USB data and status/discovery ports must differ.'}
        $autoRemove=$true;$retentionDays=1
        if($old-and $old.PSObject.Properties.Name-contains 'auto_remove_unplugged'){$autoRemove=[bool]$old.auto_remove_unplugged}
        if($old-and $old.PSObject.Properties.Name-contains 'unplugged_retention_days'){$retentionDays=[int]$old.unplugged_retention_days}
        if($retentionDays-lt 1-or $retentionDays-gt 36500){throw 'Unplugged retention must be in 1..36500 days.'}
        $hostSettings=[ordered]@{version=1;enabled=$serverEnabled;server_id=$id;name=$serverName;runtime=$runtime;profile=$Profile;allowed_ips=@($profileData.AllowedPeers);probe_address=$probe;discovery_port=$discoveryPort;auto_remove_unplugged=$autoRemove;unplugged_retention_days=$retentionDays}
        ConvertTo-BridgePortableConfig $hostSettings $Config $runtime|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $Config -Encoding utf8
        $rule=Get-NetFirewallRule -Name 'USBBridge-Discovery' -ErrorAction SilentlyContinue
        if($rule){
            if(($rule|Get-NetFirewallApplicationFilter).Program-ne $agent){throw 'The discovery firewall rule belongs to another installation.'}
            $rule|Set-NetFirewallRule -RemoteAddress $profileData.AllowedPeers -LocalPort $discoveryPort
        }else{New-NetFirewallRule -Name 'USBBridge-Discovery' -DisplayName 'USB to Network Bridge: discovery' -Direction Inbound -Action Allow -Protocol UDP -LocalPort $discoveryPort -RemoteAddress $profileData.AllowedPeers -Program $agent -Profile Any|Out-Null}
        $catalogRule=Get-NetFirewallRule -Name 'USBBridge-Catalog' -ErrorAction SilentlyContinue
        if($catalogRule){if(($catalogRule|Get-NetFirewallApplicationFilter).Program-ne $agent){throw 'The catalog rule belongs to another installation.'};$catalogRule|Set-NetFirewallRule -RemoteAddress $profileData.AllowedPeers -LocalPort $discoveryPort}
        else{New-NetFirewallRule -Name 'USBBridge-Catalog' -DisplayName 'USB to Network Bridge: device catalog' -Direction Inbound -Action Allow -Protocol TCP -LocalPort $discoveryPort -RemoteAddress $profileData.AllowedPeers -Program $agent -Profile Any|Out-Null}
        $mode='--host'
    }else{
        if(-not(Test-Path -LiteralPath $Config)){
            [ordered]@{version=1;server='';server_id=$null;discovery=$true;discovery_port=3241;poll_seconds=3;retry_max_seconds=30;devices=@()}|ConvertTo-Json -Depth 4|Set-Content -LiteralPath $Config -Encoding utf8
        }
        # Driver-specific socket settings, not global Windows TCP changes.
        $parameters='HKLM:\SYSTEM\CurrentControlSet\Services\usbip2_ude\Parameters'
        if(Test-Path -LiteralPath $parameters){
            foreach($setting in @(@('TCP_KEEPIDLE',60),@('TCP_KEEPCNT',12),@('TCP_KEEPINTVL',15))){New-ItemProperty -LiteralPath $parameters -Name $setting[0] -Value $setting[1] -PropertyType DWord -Force|Out-Null}
        }
        $mode='--client'
    }
    $settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 255 -RestartInterval (New-TimeSpan -Minutes 1)
    $action=New-ScheduledTaskAction -Execute $agent -Argument ('--supervise '+$mode+' --config "'+$Config+'"') -WorkingDirectory $runtime
    $principal=New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    if($existingHealth){$existingHealth|Disable-ScheduledTask|Out-Null;$existingHealth|Stop-ScheduledTask -ErrorAction SilentlyContinue}
    if($existing){$existing|Disable-ScheduledTask|Out-Null;Stop-ScheduledTask -TaskPath $taskPath -TaskName $taskName -ErrorAction SilentlyContinue}
    Stop-OwnedAgent $mode
    Register-ScheduledTask -TaskName $taskName -TaskPath $taskPath -Action $action -Trigger @((New-ScheduledTaskTrigger -AtStartup),(New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1))) -Settings $settings -Principal $principal -Description 'USBBridge recovery after Windows startup; no execution time limit.' -Force|Out-Null
    Start-ScheduledTask -TaskName $taskName -TaskPath $taskPath
    $healthAction=New-ScheduledTaskAction -Execute $agent -Argument ('--check-supervisor '+$mode+' --config "'+$Config+'"') -WorkingDirectory $runtime
    $healthSettings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 1)
    $healthTrigger=New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $healthName -TaskPath $taskPath -Action $healthAction -Trigger $healthTrigger -Settings $healthSettings -Principal $principal -Description 'Check role supervisor and controller heartbeat every minute; respect disabled tasks.' -Force|Out-Null
    if(-not $NoTray){
        $trayAction=New-ScheduledTaskAction -Execute $agent -Argument ('--tray --role '+$Role.ToLowerInvariant()+' --config "'+$Config+'"') -WorkingDirectory $runtime
        $trayPrincipal=New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskName $trayName -TaskPath $taskPath -Action $trayAction -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $identity.Name) -Settings $settings -Principal $trayPrincipal -Description 'USBBridge status notification icon.' -Force|Out-Null
        $shell=New-Object -ComObject WScript.Shell
        $link=$shell.CreateShortcut($shortcut)
        if(Test-Path -LiteralPath $launcher -PathType Leaf){$link.TargetPath=$launcher;$link.Arguments='--config "'+$Config+'"';$link.WorkingDirectory=$launchRoot}
        else{$link.TargetPath=$agent;$link.Arguments='--gui --role '+$Role.ToLowerInvariant()+' --config "'+$Config+'"';$link.WorkingDirectory=$runtime}
        $link.IconLocation=$agent+',0';$link.Save()
        if(Test-Path -LiteralPath $legacyShortcut){if($shell.CreateShortcut($legacyShortcut).TargetPath-eq $agent){Remove-Item -LiteralPath $legacyShortcut -Force}}
        Start-ScheduledTask -TaskName $trayName -TaskPath $taskPath -ErrorAction SilentlyContinue
    }
    [ordered]@{Success=$true;Enabled=$true;Role=$Role;Task=($taskPath+$taskName);Config=(ConvertTo-BridgePortablePath $Config $statusPath $runtime);RebootIssued=$false}|ConvertTo-Json|Set-Content -LiteralPath $statusPath -Encoding utf8
}catch{
    [ordered]@{Success=$false;Role=$Role;Error=$_.Exception.Message}|ConvertTo-Json|Set-Content -LiteralPath $statusPath -Encoding utf8
    exit 1
}
