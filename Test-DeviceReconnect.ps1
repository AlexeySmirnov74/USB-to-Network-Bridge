# Targeted hardware test: never disables a hub or an entire USB controller.
param(
    [Parameter(Mandatory=$true)][string]$DeviceInstanceId,
    [ValidateRange(5,30)][int]$HoldSeconds=10,
    [string]$ResultFile=(Join-Path $PSScriptRoot 'logs\device-reconnect.local.json'),
    [switch]$Elevate
)
$ErrorActionPreference='Stop'
if($DeviceInstanceId-notmatch '^USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}\\[^\\]+$'){throw 'Select one complete physical USB device instance, not a hub/controller/interface.'}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$ResultFile=[IO.Path]::GetFullPath($ResultFile)
if(-not $admin){
    if(-not $Elevate){throw 'Run this targeted hardware test as Administrator, or pass -Elevate.'}
    $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-DeviceInstanceId',('"'+$DeviceInstanceId+'"'),'-HoldSeconds',$HoldSeconds,'-ResultFile',('"'+$ResultFile+'"') -PassThru
    $child.WaitForExit()
    if(Test-Path -LiteralPath $ResultFile){Get-Content -LiteralPath $ResultFile -Raw}
    if($child.ExitCode-ne 0){throw 'Targeted reconnect test failed.'}
    exit 0
}
$device=Get-PnpDevice -InstanceId $DeviceInstanceId -PresentOnly -ErrorAction Stop
New-Item -ItemType Directory -Path (Split-Path -Parent $ResultFile) -Force|Out-Null
$disabled=$false
$result=[ordered]@{DeviceInstanceId=$DeviceInstanceId;StartedUtc=[DateTime]::UtcNow.ToString('o');DisabledUtc=$null;EnabledUtc=$null;Success=$false;Error=$null}
try{
    $service=(Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName DEVPKEY_Device_Service -ErrorAction Stop).Data
    if($service-eq 'VBoxUSB'){throw 'Soft-disable is unsafe for a captured VBoxUSB device. Unplug/replug its cable for the recovery test.'}
    $disabled=$true
    Disable-PnpDevice -InstanceId $device.InstanceId -Confirm:$false -ErrorAction Stop
    $result.DisabledUtc=[DateTime]::UtcNow.ToString('o')
    Start-Sleep -Seconds $HoldSeconds
}catch{$result.Error=$_.Exception.Message}finally{
    if($disabled){
        try{Enable-PnpDevice -InstanceId $device.InstanceId -Confirm:$false -ErrorAction Stop;$result.EnabledUtc=[DateTime]::UtcNow.ToString('o');$result.Success=[bool]$result.DisabledUtc}catch{$result.Error=$_.Exception.Message}
    }
    $result|ConvertTo-Json|Set-Content -LiteralPath $ResultFile -Encoding utf8
}
if(-not $result.Success){exit 1}
