# Installing USB to Network Bridge

Windows 11 x64 is the initial target. The MSI and project EXEs are intentionally unsigned. USB drivers are downloaded separately from upstream and checked against pinned SHA-256 and Authenticode. No test signing or automatic Windows restart is used.

## MSI

1. Open `USB-to-Network-Bridge-<version>-windows-x64.msi`. Choose Server on the computer with physical USB devices and Client on the computer that will use them. Both roles may be installed.
2. Open **USB to Network Bridge → Server** or **Client** in the Start menu. The first launch requests administrator rights to configure the selected role, install missing signed drivers and register startup tasks. Subsequent normal launches do not run setup again.
3. Restart Windows manually if driver setup requests it. Start menu shortcuts remain available.
4. On the server, open Settings and set the allowed client LAN/VPN IP addresses. A fresh server starts disabled with only loopback permitted and no devices shared. Enable the server, then share selected devices.
5. On the client, discover the server or enter its IP/hostname and ports, then connect the required devices.

Code and helpers reside in `%ProgramFiles%\USB to Network Bridge`. Role configuration is stored under `%ProgramData%\USB to Network Bridge\Server` or `Client`. Server configuration changes use the elevated helper; GUI language preferences and normal-user diagnostic fallback logs are under `%LocalAppData%\USB to Network Bridge`. Client configuration is writable by local users; use a trusted Windows account.

The setup refuses to take over a role whose existing startup task belongs to another executable path. An installed copy and a portable copy should not control the same role concurrently.

## Repair, update and remove

Use Installed apps or `msiexec`:

```powershell
msiexec /i USB-to-Network-Bridge-<version>-windows-x64.msi
msiexec /fa "{PRODUCT-CODE}"
msiexec /x "{PRODUCT-CODE}"
```

The product code is listed in the MSI's build metadata and Windows Installer registration. Installer maintenance stops only tasks/processes belonging to its own executable path. Repair/update resumes the installation's previously running roles. Uninstall removes its files, owned startup tasks and owned firewall rules; private configuration/logs and shared upstream USB driver installations are retained. A live device session belonging to the updated/uninstalled installation is interrupted.

## Portable ZIP

After portable setup, double-click `USB-to-Network-Bridge-Server.exe` or `USB-to-Network-Bridge-Client.exe` to open the corresponding GUI without a console window. Saved role settings are reused, and an existing interface is activated instead of starting a second instance. The launchers must stay with the complete application directory; an optional `--config FILE` selects an existing custom configuration.

Extract the application ZIP to a writable directory and follow the role-specific commands in the [English README](../README.md#quick-start) or its translated version. Keep the PowerShell helpers beside the EXEs. Driver setup and startup changes require administrator rights.

## Developer qualification

See [build and release guide](production-readiness.md) for installer testing, resource/camera observations and asset verification. Qualification commands are developer tools, not part of normal first-run configuration.
