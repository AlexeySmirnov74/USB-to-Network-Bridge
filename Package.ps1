# Packages only the applications and setup scripts. Drivers remain upstream downloads.
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$localCargo = Join-Path $root 'tools\cargo\bin\cargo.exe'
if (Test-Path -LiteralPath $localCargo) {
    $env:RUSTUP_HOME = Join-Path $root 'tools\rustup'
    $env:CARGO_HOME = Join-Path $root 'tools\cargo'
    $cargo = $localCargo
} else { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
Push-Location $root
try {
    $metadataText = & $cargo metadata --format-version 1 --locked --offline --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Build the applications before packaging.' }
    $metadata = $metadataText | ConvertFrom-Json
    $project = $metadata.packages | Where-Object name -eq 'usbbridge'
    $name = 'USB-to-Network-Bridge-' + $project.version + '-windows-x64'
    $dist = Join-Path $root 'dist'
    $stage = Join-Path $dist ($name + '-' + [guid]::NewGuid().ToString('N'))
    $licenses = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $licenses -Force | Out-Null
    foreach ($binary in @('usbbridge.exe', 'usbbridge-client.exe', 'usbbridge-server.exe', 'usbbridge-agent.exe', 'USB-to-Network-Bridge-Server.exe', 'USB-to-Network-Bridge-Client.exe')) {
        Copy-Item -LiteralPath (Join-Path $metadata.target_directory ('release\' + $binary)) -Destination $stage
    }
    foreach ($file in @(
        'Open-Installation.ps1', 'Configure-Installation.ps1', 'Maintain-Installation.ps1', 'README.md', 'README.ru.md', 'README.fr.md', 'README.de.md', 'README.zh-CN.md',
        'THIRD-PARTY.txt', 'LICENSE', 'Switch-Backend.ps1', 'Restart-UsbPort.ps1',
        'Inspect-Host.ps1', 'Download-Drivers.ps1', 'Install-ProductionDrivers.ps1',
        'driver-installers.json', 'server.example.psd1', 'client.example.json', 'Test-Transport.ps1',
        'Set-Autostart.ps1', 'Ensure-Exports.ps1', 'Test-DeviceReconnect.ps1', 'Configure-Server.ps1', 'Set-ServerState.ps1', 'Publish-Device.ps1', 'Write-DebugEvent.ps1', 'New-BackendMutex.ps1', 'Native-Capture.ps1'
    )) { Copy-Item -LiteralPath (Join-Path $root $file) -Destination $stage }
    Copy-Item -LiteralPath (Join-Path $root 'docs') -Destination $stage -Recurse
    Copy-Item -LiteralPath (Join-Path $root 'assets') -Destination $stage -Recurse
    foreach ($dependency in @($metadata.packages | Where-Object name -ne 'usbbridge')) {
        $folder = Split-Path -Parent $dependency.manifest_path
        $files = @(Get-ChildItem -LiteralPath $folder -File | Where-Object Name -Match '^LICENSE')
        if ($files.Count -eq 0) { throw ('Missing dependency license: ' + $dependency.name) }
        foreach ($file in $files) {
            Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $licenses ($dependency.name + '-' + $dependency.version + '-' + $file.Name))
        }
    }
    $hashes = @(Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object {
        [PSCustomObject]@{
            File = $_.FullName.Substring($stage.Length + 1).Replace('\', '/')
            Bytes = $_.Length
            SHA256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }
    })
    $hashes | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $stage 'SHA256.json') -Encoding UTF8
    $archive = Join-Path $dist ($name + '.zip')
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive -Force
    foreach ($launcher in @('USB-to-Network-Bridge-Server.exe', 'USB-to-Network-Bridge-Client.exe')) {
        Copy-Item -LiteralPath (Join-Path $stage $launcher) -Destination (Join-Path $dist $launcher) -Force
    }
    Get-Item -LiteralPath $archive | Select-Object FullName, Length
} finally { Pop-Location }
