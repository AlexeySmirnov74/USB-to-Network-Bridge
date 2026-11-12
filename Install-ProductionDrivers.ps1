# Install only the signed installers bundled with this release; no downloads.
param(
    [Parameter(Mandatory = $true)][ValidateSet('Server', 'Client')][string]$Role,
    [string]$InstallerDirectory = (Join-Path $PSScriptRoot 'tools\driver-installers'),
    [string[]]$AllowedPeer = @(),
    [string]$ClientDirectory = (Join-Path $env:ProgramFiles 'USBip')
)
$ErrorActionPreference = 'Stop'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { throw 'Run Install-ProductionDrivers.ps1 as Administrator.' }
if ($Role -eq 'Server') {
    if ($AllowedPeer.Count -eq 0) { throw 'Server installation requires explicit -AllowedPeer client IP addresses.' }
    foreach ($address in $AllowedPeer) { [System.Net.IPAddress]::Parse($address) | Out-Null }
    if (Get-Service usbipd, VBoxUSBMon -ErrorAction SilentlyContinue) {
        throw 'An existing usbipd/VirtualBox installation was found. Use Inspect-Host.ps1 and the upstream installer to manage it.'
    }
} elseif (Get-Service usbip2_ude, usbip2_filter -ErrorAction SilentlyContinue) {
    throw 'USBIP-WIN2 drivers are already installed. Use Inspect-Host.ps1 and the upstream installer to manage them.'
}
$verified = @(& (Join-Path $PSScriptRoot 'Verify-DriverInstaller.ps1') -Role $Role -InstallerDirectory $InstallerDirectory)
if (-not $? -or $verified.Count -ne 1 -or -not $verified[0].OfflineVerified) { throw 'Bundled installer verification failed. No download was attempted.' }
$installer = [string]$verified[0].Path
$logs = Join-Path $PSScriptRoot 'logs'
New-Item -ItemType Directory -Path $logs -Force | Out-Null
if ($Role -eq 'Server') {
    $operation = Start-Process msiexec.exe -ArgumentList @(
        '/i', ('"' + $installer + '"'), '/qn', '/norestart', '/l*v',
        ('"' + (Join-Path $logs 'server-install.log') + '"')
    ) -WindowStyle Hidden -Wait -PassThru
} else {
    $operation = Start-Process $installer -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/RESTARTEXITCODE=3010',
        '/SP-', '/NOICONS', '/TYPE=custom', '/COMPONENTS=main,client', '/TASKS=vcredist',
        ('/DIR="' + $ClientDirectory + '"'), ('/LOG="' + (Join-Path $logs 'client-install.log') + '"')
    ) -WindowStyle Hidden -Wait -PassThru
}
if ($operation.ExitCode -notin @(0, 3010)) { throw ('Upstream installer failed: ' + $operation.ExitCode) }
if ($Role -eq 'Server') {
    try {
        Get-NetFirewallRule -DisplayName 'usbipd' -ErrorAction Stop | Set-NetFirewallRule -RemoteAddress $AllowedPeer -ErrorAction Stop
    } catch {
        Get-NetFirewallRule -DisplayName 'usbipd' -ErrorAction SilentlyContinue | Disable-NetFirewallRule
        throw
    }
}
[PSCustomObject]@{
    Role = $Role
    ExitCode = $operation.ExitCode
    RestartRequired = ($operation.ExitCode -eq 3010)
    RestartIssued = $false
}
if ($operation.ExitCode -eq 3010) { Write-Output 'Restart Windows manually before testing USB devices.' }
