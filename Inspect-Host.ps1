param([string]$PackageDirectory=$PSScriptRoot)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class UsbBridgeHostInspection {
 [StructLayout(LayoutKind.Sequential)] public struct CodeIntegrity {public uint Length,Options;}
 [DllImport("ntdll.dll")] static extern int NtQuerySystemInformation(int kind,ref CodeIntegrity value,int size,IntPtr returned);
 public static uint Options(){var value=new CodeIntegrity{Length=8};if(NtQuerySystemInformation(103,ref value,8,IntPtr.Zero)<0)throw new InvalidOperationException("Cannot query Windows code integrity");return value.Options;}
}
'@
$os=Get-CimInstance Win32_OperatingSystem
$secureBoot=$null
try {$secureBoot=Confirm-SecureBootUEFI -ErrorAction Stop} catch {}
if($null-eq $secureBoot){
    $secureBootState=Get-ItemProperty -LiteralPath HKLM:\SYSTEM\CurrentControlSet\Control\SecureBoot\State -ErrorAction SilentlyContinue
    if($null-ne $secureBootState){$secureBoot=[bool]$secureBootState.UEFISecureBootEnabled}
}
$options=[UsbBridgeHostInspection]::Options()
[ordered]@{
    Computer=$env:COMPUTERNAME
    OS=$os.Caption
    Version=$os.Version
    Architecture=$os.OSArchitecture
    Administrator=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    TestSigningEnabled=($options-band 2)-ne 0
    CodeIntegrityOptions=$options
    MemoryIntegrityEnabled=($options-band 1024)-ne 0
    SecureBoot=$secureBoot
    PackageDirectory=$PackageDirectory
    DirectoryExists=Test-Path -LiteralPath $PackageDirectory
    PackageFiles=@(if(Test-Path -LiteralPath $PackageDirectory){Get-ChildItem -LiteralPath $PackageDirectory -Force|Select-Object Name,Length,Mode})
    Drivers=@(Get-CimInstance Win32_SystemDriver -Filter "Name='UsbDk' OR Name='usbip2_ude' OR Name='usbip2_filter' OR Name='VBoxUSB' OR Name='VBoxUSBMon'"|Select-Object Name,State,PathName)
    Cameras=@(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue|Where-Object {$_.Class-in @('Camera','Image')}|Select-Object Status,FriendlyName,InstanceId)
    Addresses=@(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue|Where-Object {$_.IPAddress-ne '127.0.0.1'}|Select-Object InterfaceAlias,IPAddress)
}|ConvertTo-Json -Depth 5
