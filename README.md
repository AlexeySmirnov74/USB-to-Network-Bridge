# USB to Network Bridge

[![Version](https://img.shields.io/github/v/release/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github&label=version)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/AlexeySmirnov74/USB-to-Network-Bridge/ci.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white&label=build)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml)
[![Windows 11 x64](https://img.shields.io/badge/platform-Windows%2011%20x64-0078D4?style=flat-square)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge#quick-start)
[![Rust 2024](https://img.shields.io/badge/Rust-2024%20edition-CE422B?style=flat-square&logo=rust&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square&logo=opensourceinitiative&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/stargazers)

[English](README.md) · [Français](README.fr.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [简体中文](README.zh-CN.md)

An open-source USB over IP (USB/IP) redirector for Windows 11 x64. Share real USB devices between Windows computers over a LAN or VPN, including webcams and Android phone ADB/storage. A Rust server exports selected devices; a Windows client imports them through a virtual USB controller. No activation, trial timer or license-based device limit.

**v0.2.2 — Windows 11 x64.** A Rust USB server and client with a desktop GUI, command-line control, automatic discovery and reconnect, plus an MSI installer and portable ZIP. [Download the latest release](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest).

**Agent health in v0.2.1:** server and client controllers run under separate supervisors. Exited controllers restart automatically; stopped heartbeat progress is detected after two minutes. Independent one-minute scheduled checks also recover a missing or stalled supervisor. Stale reports no longer claim a connected device. Use `usbbridge --config FILE server health` or `client health` to inspect current health. See [changes](CHANGELOG.md).

![USB devices connected to a Windows server, transported over a trusted network, and used by Windows clients](docs/assets/architecture.svg)

## USB camera from a remote video source

**New in v0.2.2:** send video from a Windows client into a USB/UVC camera on the Windows server. Choose **Server → USB camera — receive video**, set its name (for example `4k web camera`), then configure **Device → Send video** on the client with the source, server address and the same connection key. The server uses the signed USBIP-WIN2 controller and Windows `usbvideo.sys`; the camera has no mandatory Media Foundation name suffix. Installed OBS/Unity Capture sources are selected through DirectShow. See the [video camera quick start](docs/virtual-camera.md).

The video path uses authenticated TCP/MJPEG and retains the USB camera while a lost source reconnects, showing black instead of switching to another camera. Each endpoint has startup recovery and an independent heartbeat check. The name is configurable independently of resolution; 1280×720 at approximately 30 FPS was measured with a real camera source. Use a trusted LAN or VPN: frame authentication does not encrypt the images. The virtual USB controller remains visible in the Windows device tree.

## What you can do

- Discover servers automatically or add an IP/hostname and port; save multiple servers and reconnect selected devices after a restart or cable reconnection.
- Share and unshare individual devices without restarting unrelated USB sessions. Each device has one remote owner at a time.
- Choose the stock `usbipd-win` server or the Rust `Native` transport. Both use the upstream signed VBoxUSB capture driver. The Windows client uses the upstream signed USBIP-WIN2 virtual controller.
- Use a dark desktop GUI, tray menus, or the [command line](docs/command-line.md). English is the default; French, Russian, German, and Simplified Chinese are included.
- Configure direct access, HTTP CONNECT, HTTPS CONNECT, or SOCKS5 per server, with optional credentials protected locally by Windows DPAPI.
- Test a selected webcam with live received/decoded FPS. On the client, Low-light compensation is saved per camera; enabling it can lower FPS. Camera Test blocks its main window until closed, on both roles.
- Configure Windows startup, ports, IP allowlists, per-device local use while idle, and automatic cleanup of unplugged entries (one day by default).

**“Enabled” in the server table means local use without a client**, independently of Share. Leave it off to reserve a webcam for remote access. Local and remote applications cannot use the same captured device simultaneously.

## usbipd-win and Native Rust

These are two **server backends**, rather than two different kernel drivers. [usbipd-win](https://github.com/dorssel/usbipd-win) provides the upstream USB/IP service and device-management tools; **Native** is this project's own Rust capture and USB/IP server.

| Component | Stock usbipd-win | Native Rust |
| --- | --- | --- |
| Server handling USB traffic | Upstream usbipd-win service, implemented in C#/.NET | Our Rust executable, usbbridge-server.exe |
| Physical USB capture | Upstream production-signed VBoxUSB driver | The same unmodified, production-signed VBoxUSB driver |
| Server data path | USB device → VBoxUSB → usbipd-win service → TCP | USB device → VBoxUSB → Rust server → TCP |
| Transfer implementation | Maintained by the upstream usbipd-win project | Direct asynchronous IOCTL/IOCP capture, pipelined requests, bounded buffers/queues and TCP_NODELAY |
| Windows client | Our client controls the signed USBIP-WIN2 virtual controller | The same client and USBIP-WIN2 controller |
| Dependency on usbipd-win | Supplies the stock server and device management | Still needed for installation, inventory and device reservation; its service does not forward Native USB traffic |

Native lets us tune the USB/network path directly. For example, initializing the capture driver's device-speed information before claiming a camera restored approximately **30 FPS** for the tested Chicony 1280 × 720 MJPEG mode. This is a measured device-specific result, not a guarantee that Native is faster for every device. See [performance measurements](docs/performance.md) and [architecture](docs/architecture.md).

Discovery, GUI/tray controls, saved selections, automatic reconnect and proxy support belong to USB to Network Bridge and are available with either backend. Switch the backend in server Settings; switching reconnects active devices. Neither backend requires Test Signing or replacing the signed kernel driver.

## Screenshots

### Android phone connected on the Windows client

![Android phone connected through USB to Network Bridge, with ADB Interface in Device Manager and phone storage in File Explorer](docs/assets/connect-phone.jpg)

The client has imported a realme GT 8 Pro. Windows exposes its ADB interface and internal storage on the remote computer.

### Webcam preview and FPS on the Windows client

![USB webcam imported on the Windows client with Camera Test showing the live preview and FPS](docs/assets/connect-camera.jpg)

1280 × 720 MJPEG in the 30 FPS mode. Camera Test shows **30.15 received FPS and 30.15 decoded FPS**, with a **29.84 FPS average**; Low-light compensation is off.

For a concise product description and common use cases, see the [project FAQ](docs/project-overview.md).

## Quick start

You need two Windows 11 x64 computers, administrator access for driver/setup operations, and a trusted network or VPN. Secure Boot can remain enabled: no test signing is required for the upstream drivers. Only publish devices that may be handed over to another computer; capturing a mouse, keyboard, or disk removes its local availability.

### 1. Get the applications

Download the MSI or portable ZIP from [Releases](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest). The [MSI guide](docs/installation.md) covers role selection and Start menu first-run setup.

For portable deployment, extract the application ZIP into a writable directory such as `C:\USBBridge` on both computers. Run the commands below from that directory and keep the `.ps1` helpers beside the EXEs. Developer builds are also available from successful [CI runs](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml).


### 2. Set up the server

After setup, double-click **`USB-to-Network-Bridge-Server.exe`** on the server or **`USB-to-Network-Bridge-Client.exe`** on the client. Both open the correct GUI without a console window and reuse saved settings. Keep them with the complete application folder. The MSI Start menu shortcuts use these launchers too; `usbbridge.exe` remains the command-line tool.

[Download Server EXE](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Server.exe) · [Download Client EXE](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Client.exe). Place individual EXE downloads in the complete application folder described above.

In **Windows PowerShell as Administrator**, on the computer with the physical devices:

```powershell
Set-Location C:\USBBridge
# Replace the documentation address with the actual client's LAN/VPN IP.
.\Install-ProductionDrivers.ps1 -Role Server -AllowedPeer 192.0.2.10
Copy-Item .\server.example.psd1 .\server.local.psd1
notepad .\server.local.psd1
```

Set `AllowedPeers` to the actual client IP address(es). Keep `Export = @()` for the first start; select devices in the GUI afterwards. If the upstream installer requests a restart, restart manually before continuing. Existing driver installations are not overwritten: inspect/manage them with `Inspect-Host.ps1` and the upstream installer instead of reinstalling blindly.

```powershell
.\Switch-Backend.ps1 -Backend Native -RuntimeDirectory C:\USBBridge
.\Set-Autostart.ps1 -Role Server -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\host.local.json -Profile C:\USBBridge\server.local.psd1
.\USB-to-Network-Bridge-Server.exe
```

The tray icon and GUI show server status. Right-click a device → **Share**, or double-click an unshared device row. To use the stock transport, switch the server module in Settings or run `Switch-Backend.ps1 -Backend Usbipd`; changing the transport reconnects active devices.

### 3. Set up the client

In **Windows PowerShell as Administrator**, on the remote computer:

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Client
Copy-Item .\client.example.json .\client.local.json
# Restart manually first if the driver installer requests it.
.\Set-Autostart.ps1 -Role Client -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\client.local.json
.\USB-to-Network-Bridge-Client.exe
```

Use **Find servers**, select a server, select a shared device, then **Connect**. If discovery cannot cross the network, use **Add server** with its LAN/VPN address. Saved device choices reconnect automatically; **Disconnect** cancels the corresponding choice. Selecting a server node allows pausing/resuming all its remembered devices without affecting other servers.

### 4. Check the connection

Open the remote device in its normal Windows application. For a webcam, right-click it → **Camera Test**. A local test on the server requires the camera to be available locally; a camera captured remotely must be tested on the client. A camera without a stable unique USB identity may need to be selected again after moving ports.

| Connection | Default | Purpose |
| --- | --- | --- |
| TCP | 3240 | USB/IP device data |
| UDP | 3241 | Server discovery on local IPv4 networks |
| TCP | 3241 | Full device catalog and status |

Ports can be changed to **1–65535**. The USB data and catalog ports must differ. Match the discovery/catalog port on clients; a proxy must permit both TCP ports. UDP discovery does not traverse a proxy.

## Operation and diagnostics

Closing the GUI leaves background work running. Startup tasks restore the server/client after Windows starts, including before user logon; an interactive tray appears after logon. One instance of each role is allowed per computer. Disable startup with `Set-Autostart.ps1 -Role Server` (or `Client`) using the same runtime/config arguments and `-Disable`.

Detailed rotating development logs are stored in `logs\*.debug.jsonl` beside the configuration: UI actions, USB identities/ports, reconnects, and errors. Video images, USB payloads, and passwords are not logged. Logs can still contain IP addresses, local paths, and device serial numbers; review them before posting publicly.

Short network interruptions trigger recovery, not an unlimited guarantee of USB session continuity. Long outages or physical removal can require a device re-enumeration. USB/IP has no built-in encryption or authenticated server identity here: use a trusted LAN/VPN and restrict the firewall/IP allowlist. Do not expose the data/catalog ports directly to the Internet. HTTPS proxy mode encrypts the client-to-proxy hop only.

## Build from source

Requirements: Windows x64, Rust stable (MSVC target, `rustfmt`, `clippy`), MSVC C++ Build Tools, and a Windows SDK. Optional `Bootstrap.ps1` installs a local Rust toolchain; `-InstallBuildTools` additionally installs Microsoft build tools.

```powershell
git clone https://github.com/AlexeySmirnov74/USB-to-Network-Bridge.git
Set-Location USB-to-Network-Bridge
.\Build.ps1
.\Package.ps1
```

`Build.ps1` checks formatting, runs Clippy and tests, and builds six release executables. `Package.ps1` creates `dist\USB-to-Network-Bridge-0.2.2-windows-x64.zip` with runtime helpers, examples, docs, and dependency licenses. Drivers are downloaded from upstream, verified against pinned SHA-256 and Authenticode in `driver-installers.json`, and are not bundled. CI builds/packages without installing USB drivers; hardware tests remain separate.

## Installation and runtime

The current package **still requires Windows PowerShell 5.1 and the supplied runtime `.ps1` helpers**. They implement privileged publication/capture, backend switching, firewall configuration, and startup tasks. `Build.ps1`, `Bootstrap.ps1`, and packaging scripts are development tools and are not needed on an installed client/server. A future native service can replace runtime scripts; deleting them now breaks the application.

The project EXEs and MSI are unsigned. USB drivers retain their official production signatures; Secure Boot and memory integrity can remain enabled. The MSI supports installation, repair, upgrade, rollback and uninstall. See [installation](docs/installation.md) and [building and releasing](docs/production-readiness.md).

## Documentation and license

[Architecture](docs/architecture.md) · [CLI](docs/command-line.md) · [Camera testing](docs/camera-preview.md) · [Performance](docs/performance.md) · [Hardware tests](docs/hardware-tests.md) · [Device identifiers](docs/device-identifiers.md) · [Memory audit](docs/memory-audit.md)

Some detailed engineering notes are currently in Russian. The quick start above and the translated READMEs are maintained together.

Project code: [MIT](LICENSE). Dependencies and external drivers retain their own licenses: [third-party notices](THIRD-PARTY.txt). Local configurations, logs, device inventories, credentials and signing keys are excluded from public packages.
