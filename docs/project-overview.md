# USB to Network Bridge: project FAQ

## What is it?

USB to Network Bridge is an open-source USB over IP (USB/IP) redirector for Windows 11 x64. A Rust server shares selected physical USB devices over a LAN or VPN; a Windows client imports them through a virtual USB controller so normal Windows software can use them.

- Repository: https://github.com/AlexeySmirnov74/USB-to-Network-Bridge
- Download: https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest
- Project code license: MIT. Dependencies and external USB drivers retain their own licenses.
- Interface languages: English, French, Russian, German and Simplified Chinese.

## What can it be used for?

Examples include using a remote USB webcam in Windows applications, connecting an Android phone's ADB interface and storage on another Windows PC, and sharing selected HID devices. The [README screenshots](../README.md#screenshots) show an imported phone and webcam. Compatibility depends on the physical device, its Windows drivers and the network; the project does not certify every device in a class.

Both endpoints can run Windows; a Linux VM or WSL is not required for the documented setup. Other client/server operating systems are not the documented hardware-tested target.

## Is Native Rust a new Windows kernel driver?

No. Native is the project's Rust userspace capture and USB/IP server. It opens the unmodified, production-signed VBoxUSB capture driver directly. The stock backend instead routes USB data through the upstream usbipd-win service. The Windows client uses the same signed USBIP-WIN2 virtual controller in both cases.

usbipd-win remains a dependency for installation, inventory and device reservation even in Native mode. See the [backend comparison](../README.md#usbipd-win-and-native-rust) and [implementation architecture](architecture.md).

## What makes the Native transport different?

The Rust server owns asynchronous IOCTL/IOCP capture, pipelined USB requests, bounded buffers/queues and TCP configuration. That makes device-specific scheduling and transfer changes possible without modifying the signed kernel driver. Measurements and their conditions are recorded in [performance](performance.md); no universal speed multiplier is claimed.

Server discovery, GUI/tray controls, automatic reconnect, multiple remembered servers and proxy support are application features available with either backend.

## Does a 30 FPS camera mode guarantee 30 received FPS?

No. Camera Test displays selected mode, received FPS, decoded FPS and an average separately. Exposure/low-light controls, device behavior and network delivery can change actual FPS. The client remembers Low-light compensation per camera. See [camera testing](camera-preview.md).

## Can several clients and servers be used?

Yes. A client can remember several servers and selected devices. Each captured physical USB device has one active remote owner at a time. Local-use-while-idle is configurable per device; simultaneous local and remote use of a captured device is not supported.

## Is it limited by activation or a trial timer?

There is no project activation, trial expiration or license-based device count. USB controller, addressing, driver and hardware limits still apply.

## How do I install it?

Download the MSI or portable Windows x64 ZIP from Releases. The MSI offers Server and Client roles; first-run setup installs missing upstream signed drivers and configures startup. Administrator rights are needed for driver/setup operations. See [installation](installation.md) and the [quick start](../README.md#quick-start).

The project's EXEs/MSI are unsigned. Upstream USB drivers keep their production signatures, so the documented setup does not require Test Signing or disabling Secure Boot. Windows PowerShell 5.1 runtime helpers are included and remain necessary.

## Which networks and proxies are supported?

Use a trusted LAN or VPN with explicit client IP allowlists. USB data uses TCP 3240 by default; catalog/discovery uses TCP/UDP 3241. Ports are configurable. Direct access, HTTP CONNECT, HTTPS CONNECT and SOCKS5 are supported; optional proxy credentials are protected locally with Windows DPAPI.

USB/IP here does not provide transport encryption or authenticated server identity. HTTPS proxy encryption covers the client-to-proxy segment. See [architecture](architecture.md) and [CLI](command-line.md).

## Where can I find source, diagnostics and compatibility information?

Source is in this repository. Downloads and source archives are in Releases. Start with [hardware observations](hardware-tests.md), [device identifiers](device-identifiers.md), [memory audit](memory-audit.md), and the [build/release guide](production-readiness.md). Local configurations and detailed logs are not public project assets.
