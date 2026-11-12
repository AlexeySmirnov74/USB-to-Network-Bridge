# Package applications plus exactly the two pinned, signed offline driver installers.
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
    # Fetching is a separate developer/CI step. Packaging is local-only and fails
    # before creating a candidate if either pinned offline installer is absent.
    $driverManifestPath = Join-Path $root 'driver-installers.json'
    $driverManifest = Get-Content -LiteralPath $driverManifestPath -Raw | ConvertFrom-Json
    $driverCache = Join-Path $root 'tools\driver-installers'
    & (Join-Path $root 'Verify-DriverInstaller.ps1') -Role Both -InstallerDirectory $driverCache -ManifestPath $driverManifestPath | Out-Null
    if (-not $?) { throw 'The pinned offline driver installers failed verification.' }
    $unityPin = Get-Content -LiteralPath (Join-Path $root 'unity-capture.json') -Raw | ConvertFrom-Json
    $unityFile = Join-Path $root $unityPin.file
    if ($unityPin.schema -ne 1 -or $unityPin.file -cne 'components/unity-capture/UnityCaptureFilter64.dll' -or $unityPin.architecture -cne 'x64' -or (Get-FileHash -LiteralPath $unityFile -Algorithm SHA256).Hash -ne $unityPin.sha256) { throw 'Missing or corrupt pinned offline Unity Capture input. Run Download-VideoComponents.ps1 at build time.' }
    $name = 'USB-to-Network-Bridge-' + $project.version + '-windows-x64'
    $dist = Join-Path $root 'dist'
    $stage = Join-Path $dist ($name + '-' + [guid]::NewGuid().ToString('N'))
    $app = Join-Path $stage 'app'
    $licenses = Join-Path $app 'licenses'
    New-Item -ItemType Directory -Path $licenses -Force | Out-Null
    foreach ($binary in @('USB-to-Network-Bridge-Server.exe', 'USB-to-Network-Bridge-Client.exe')) {
        Copy-Item -LiteralPath (Join-Path $metadata.target_directory ('release\' + $binary)) -Destination $stage
    }
    foreach ($binary in @('usbbridge.exe', 'usbbridge-client.exe', 'usbbridge-server.exe', 'usbbridge-agent.exe', 'usbbridge-camera.exe')) {
        Copy-Item -LiteralPath (Join-Path $metadata.target_directory ('release\' + $binary)) -Destination $app
    }
    foreach ($file in @(
        'Open-Installation.ps1', 'Configure-Installation.ps1', 'FirstRun-Identity.ps1', 'Portable-Paths.ps1', 'Portable-Installation.ps1', 'Update-PortableSettings.ps1', 'Relocate-Installation.ps1', 'Migrate-PortableLayout.ps1', 'Maintain-Installation.ps1', 'Restart-AgentSupervisor.ps1', 'Set-VideoMode.ps1', 'Restart-VideoEndpoint.ps1', 'Configure-UnityCapture.ps1', 'CHANGELOG.md', 'README.md', 'README.ru.md', 'README.fr.md', 'README.de.md', 'README.zh-CN.md',
        'THIRD-PARTY.txt', 'LICENSE', 'Switch-Backend.ps1', 'Restart-UsbPort.ps1', 'Quit-Application.ps1',
        'Inspect-Host.ps1', 'Ensure-VideoPrerequisites.ps1', 'unity-capture.json', 'Install-ProductionDrivers.ps1',
        'Verify-DriverInstaller.ps1', 'Ensure-ProductionDrivers.ps1', 'driver-installers.json', 'server.example.psd1', 'client.example.json', 'Test-Transport.ps1',
        'Set-Autostart.ps1', 'Ensure-Exports.ps1', 'Test-DeviceReconnect.ps1', 'Configure-Server.ps1', 'Set-ServerState.ps1', 'Publish-Device.ps1', 'Write-DebugEvent.ps1', 'New-BackendMutex.ps1', 'Set-BackendProcessAccess.ps1', 'Native-Capture.ps1'
    )) { Copy-Item -LiteralPath (Join-Path $root $file) -Destination $app }
    $unityDirectory = Join-Path $app 'components\unity-capture'
    New-Item -ItemType Directory -Path $unityDirectory -Force | Out-Null
    Copy-Item -LiteralPath $unityFile -Destination (Join-Path $unityDirectory 'UnityCaptureFilter64.dll')
    if ((Get-FileHash -LiteralPath (Join-Path $unityDirectory 'UnityCaptureFilter64.dll')).Hash -ne $unityPin.sha256) { throw 'Copied Unity Capture hash mismatch.' }
    Copy-Item -LiteralPath (Join-Path $root 'licenses\unity-capture') -Destination $licenses -Recurse
    $driverNotices = Join-Path $root 'licenses\drivers'
    foreach ($item in @(Get-ChildItem -LiteralPath $driverNotices -Recurse -Force)) {
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw ('Driver notices cannot contain links: ' + $item.FullName) }
        if (-not $item.PSIsContainer -and ($item.Name.Contains('.local.') -or $item.Extension -notin @('.md', '.txt', '.license'))) {
            throw ('Only upstream text notices belong in licenses/drivers: ' + $item.FullName)
        }
    }
    Copy-Item -LiteralPath $driverNotices -Destination $licenses -Recurse
    $packagedDrivers = Join-Path $app 'tools\driver-installers'
    New-Item -ItemType Directory -Path $packagedDrivers -Force | Out-Null
    foreach ($role in @('Server', 'Client')) {
        # Never copy a whole local tools/cache directory into a public payload.
        Copy-Item -LiteralPath (Join-Path $driverCache $driverManifest.$role.file) -Destination (Join-Path $packagedDrivers $driverManifest.$role.file)
    }
    & (Join-Path $root 'Verify-DriverInstaller.ps1') -Role Both -InstallerDirectory $packagedDrivers -ManifestPath $driverManifestPath | Out-Null
    if (-not $?) { throw 'Copied offline driver installers failed verification.' }
    foreach ($folder in @('docs', 'assets')) {
        $source = Join-Path $root $folder
        foreach ($item in @(Get-ChildItem -LiteralPath $source -Recurse -Force)) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw ('Public payload cannot contain links: ' + $item.FullName) }
            if (-not $item.PSIsContainer -and ($item.Name.Contains('.local.') -or $item.Extension -in @('.pfx', '.key', '.pem', '.credential', '.sys', '.cat', '.msi', '.exe', '.dll', '.jsonl', '.log'))) {
                throw ('Private/runtime file in documentation or assets: ' + $item.FullName)
            }
        }
        Copy-Item -LiteralPath $source -Destination $app -Recurse
    }
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
    # File names remain relative to the package root, including both launchers.
    # The manifest itself lives with the support files, so the visible root has
    # exactly the two application entry points and the app directory.
    $hashes | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $app 'SHA256.json') -Encoding UTF8
    # Some upstream crate archives use pre-1980 mtimes, outside ZIP's DOS range.
    foreach ($file in @(Get-ChildItem -LiteralPath $stage -Recurse -File)) {
        if ($file.LastWriteTime.Year -lt 1980 -or $file.LastWriteTime.Year -gt 2107) {
            $file.LastWriteTime = [DateTime]::new(1980, 1, 1, 0, 0, 0)
        }
    }
    $archive = Join-Path $dist ($name + '.zip')
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive -Force
    foreach ($launcher in @('USB-to-Network-Bridge-Server.exe', 'USB-to-Network-Bridge-Client.exe')) {
        Copy-Item -LiteralPath (Join-Path $stage $launcher) -Destination (Join-Path $dist $launcher) -Force
    }
    foreach ($role in @('Server', 'Client')) {
        # Standalone upstream assets are also published with the GitHub release.
        Copy-Item -LiteralPath (Join-Path $packagedDrivers $driverManifest.$role.file) -Destination (Join-Path $dist $driverManifest.$role.file) -Force
    }
    Get-Item -LiteralPath $archive | Select-Object FullName, Length
} finally { Pop-Location }
