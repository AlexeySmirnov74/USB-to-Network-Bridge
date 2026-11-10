# Building and releasing

Project EXEs and MSI are unsigned. Upstream USB drivers keep their production signatures. Windows PowerShell 5.1 and the supplied runtime/setup helpers are part of the application.

## Build

Use Windows x64, Rust stable MSVC with rustfmt/Clippy, MSVC C++ Build Tools, Windows SDK and Python 3:

~~~powershell
.\Build.ps1
py -3 -m unittest discover -s tests -p 'test_*.py'
.\Package.ps1
.\Build-Installer.ps1
.\Test-Installer.ps1 -Elevate
~~~

The MSI builder uses Windows Installer and makecab.exe. Installer tests use a separate product identity and installation folder. They cover app installation, repair, upgrade, injected-failure rollback, downgrade refusal and uninstall, without installing USB drivers.

CI builds the application, runs automated and installer lifecycle tests, and uploads the MSI, portable ZIP and source ZIP.

## Files needed at runtime

- Runtime: Publish-Device.ps1, Configure-Server.ps1, Set-ServerState.ps1, Switch-Backend.ps1, Ensure-Exports.ps1, Native-Capture.ps1, Restart-UsbPort.ps1, New-BackendMutex.ps1 and Write-DebugEvent.ps1.
- Setup and maintenance: Open-Installation.ps1, Configure-Installation.ps1, Maintain-Installation.ps1, Restart-AgentSupervisor.ps1, Set-Autostart.ps1, Download-Drivers.ps1, Install-ProductionDrivers.ps1 and Inspect-Host.ps1.
- Build and qualification scripts are development tools. Removing runtime helpers from an installed copy breaks operation.

## Development verification

Start-Qualification.ps1 observes running components and can continuously receive/decode a camera without saving images. Stop-Qualification.ps1 requests cooperative shutdown. Analyze-Qualification.py checks actual duration, frame/error counters, configuration and executable hashes, restarts and process resource growth. It never rewrites the original evidence.

Maintain private hardware reports for cable replug/port changes, network interruptions, sleep/resume, camera controls/identifiers and fresh-machine role setup. Keep configurations, credentials, device serials, logs and local toolchains out of Git.

## Preparing assets

Review and commit public sources before packaging. Package-Source.py archives reviewed Git blobs. Prepare-Release.ps1 verifies the selected validation profile and executable hashes, then compares every portable/MSI payload file and source ZIP entry. Detailed verification records stay in ignored local logs.

The public release assets are the two desktop launcher EXEs, MSI, portable ZIP, source ZIP, release-manifest.json and SHA256SUMS.txt. Each published update gets a new version and matching tag/release; CHANGELOG.md describes the actual changes. Preserve previous release assets and tags. The manifest records the source commit and artifact hashes. Publication preserves Git history and uses a versioned GitHub Release.

## Installation and migration

See [installation](installation.md). Installer maintenance affects only tasks, processes and firewall rules owned by its exact executable path. USB driver installations and private role data are retained during application removal.

For migration, back up private configuration and owned task/firewall settings. Stop the old installation explicitly before configuring the new role. Update runtime/profile/config paths and startup actions together; moving a live server interrupts its own device sessions. Do not ship toolchain compatibility junctions.
