# Video source to USB/UVC camera

USB to Network Bridge v0.2.2 adds a video path in the client-to-server direction. A software UVC device on the receiving Windows PC is imported through a local USB/IP connection using the existing production-signed USBIP-WIN2 controller. Windows loads its inbox `usbvideo.sys` camera driver. The source can be an installed DirectShow camera, OBS Virtual Camera, Unity Capture, or the built-in test pattern.

```mermaid
flowchart LR
    A[Client: OBS / Unity / camera] --> B[Capture and MJPEG encoding]
    B -->|Authenticated TCP| C[Server: latest complete frame]
    C --> D[Local USB/UVC endpoint]
    D --> E[USBIP-WIN2 and usbvideo.sys]
    E --> F[Video application]
```

For a third-party producer running on the client PC, see the [OBS/Unity Capture integration guide](third-party-video.md) ([Русский](third-party-video.ru.md)). It covers source setup, interactive sessions, exact source identity, CLI integration and verification.

## Quick start

1. Install the complete application package on both Windows 11 x64 PCs. The receiving server also needs the signed **USBIP-WIN2 client driver**, because it hosts the virtual USB controller. If this PC only has the physical-sharing server driver, run `app\Install-ProductionDrivers.ps1 -Role Client` from the package root in an elevated PowerShell window. This installs the receiving USB controller without configuring another application role. Keep Secure Boot enabled; follow an upstream installer's request for a manual restart when needed.
2. On the server, open **Server → USB camera — receive video**. Enable the mode, set a name such as `4k web camera`, choose a resolution, and set the incoming video TCP port (default **13251**). Enter a connection key of 16–128 characters. The receiver uses the server's allowed client IP addresses when saving this dialog.
3. On the client, open **Device → Send video**. Enable the mode, select a source, enter `SERVER_IP:13251`, and enter the same connection key. The test pattern provides a source without opening a physical camera. For OBS, start **Virtual Camera** in OBS first. The complete Bridge package includes the x64 Unity Capture filter and client setup registers **Unity Capture Server** (slot 0) and **Unity Capture Client** (slot 1) locally; select the intended slot and start the producer. To check or repair registration, close video applications and run `Ensure-VideoPrerequisites.ps1 -Role Client -RuntimeDirectory . -Elevate` from `app` in 64-bit Windows PowerShell. OBS and the producer application are installed separately.
4. On the server, select the configured camera name in Camera Test or the target video application. Confirm that the application has Windows camera permission. Camera Test reports frames actually received and decoded.

The GUI stores connection keys using Windows DPAPI in protected files. A blank key field preserves the current key. The sender starts in the selected interactive Windows user's session so that desktop OBS/Unity sources can be accessed; the receiver runs as SYSTEM for the virtual USB controller. Enabled modules restart at Windows startup/user logon and have independent one-minute health checks. A stopped heartbeat is recovered after 120 seconds plus the check interval. Disabling the mode also disables its recovery tasks.

## Behavior

- The camera's name is independent of its resolution. Calling it `4k web camera` does not turn a 720p source into a native 4K source. GUI modes advertise 30 FPS; measured source and output rates depend on the producer, encoding cost and network.
- Changing the name preserves the serial identity and updates only the owned camera's PnP name and its capture, video and Media Foundation camera interface names. Applying a name or resolution change reconnects this virtual camera; an application may need to reopen it.
- Network/source loss leaves the receiving USB camera registered. After two seconds without a fresh image it supplies black, then resumes the selected source after reconnection. It never falls back to a different physical camera.
- Video reception is configured independently of physical USB sharing. The generated camera is managed from its video dialog and is excluded from the physical publication table, preventing a capture loop.
- The video channel uses TCP/MJPEG with mutual authentication and per-frame HMAC-SHA256 integrity/replay protection. Images are not encrypted by this channel; use a trusted LAN or VPN. Its port and key are separate from USB data/discovery and USB proxy settings.
- The receiving USB/IP listener is local-only (`127.0.0.1:13250` by default). Incoming network video is limited to the configured client IPs. One video sender owns the receiver at a time. Frame/request queues and JPEG sizes are bounded; diagnostics do not store images or keys.
- This remains an emulated device: its virtual USB controller is visible in the Windows device tree. The software endpoint currently uses prototype VID:PID **1209:F002**; no assigned physical USB product ID is claimed.

## Command line

The packaged CLI and PowerShell helpers are in `app`. First use
`Set-Location C:\USBBridge\app` (adjust for your installation), then pass the actual
host/client configuration paths used by the launcher or its startup tasks:

```powershell
.\usbbridge.exe --config C:\path\host.local.json server video settings
.\usbbridge.exe --config C:\path\host.local.json server video status
.\usbbridge.exe --config C:\path\host.local.json server video configure C:\path\receive.json
.\usbbridge.exe --config C:\path\host.local.json server video off
.\usbbridge.exe --config C:\path\host.local.json server video on
.\usbbridge.exe --config C:\path\client.local.json client video configure C:\path\send.json
```

A receiver settings file:

```json
{
  "enabled": true,
  "camera": {
    "name": "4k web camera",
    "serial": "UBC-your-stable-software-identity",
    "width": 1280,
    "height": 720,
    "fps": 30,
    "test_pattern": false
  },
  "usb_port": 13250,
  "video_port": 13251,
  "allowed_ips": ["192.0.2.20"],
  "key_file": "C:\\path\\video.local.key"
}
```

A sender settings file:

```json
{
  "enabled": true,
  "server": "192.0.2.10:13251",
  "camera": "test-pattern",
  "quality": 85,
  "key_file": "C:\\path\\video.local.key"
}
```

For an actual source, replace `test-pattern` with its exact path/USB identity from `usbbridge --config FILE client cameras`. Source enumeration never selects another device merely because its name matches. The source is scaled with its aspect ratio preserved when needed.

`usbbridge-camera keygen C:\path\video.local.key` creates a new private export key file and refuses to overwrite one. Transfer that same key securely to the other PC. Keep key files outside the repository. The GUI uses encrypted `.credential` files instead; `key_file` accepts either representation on its originating Windows PC.

For an isolated local test, `usbbridge-camera serve --name "4k web camera" --listen 127.0.0.1:13250` publishes a test-pattern UVC endpoint. `usbbridge-camera attach 127.0.0.1:13250` imports it through the installed controller; `detach PORT` removes only an emulated camera port. Standalone `serve` is a diagnostic endpoint; normal startup/recovery uses the GUI or `video configure`.

## Measured fixture

A real 2K FHD camera was already imported on the client through the physical USB bridge. Its video was sent back over the LAN to the server's generated USB/UVC camera. At **1280×720 MJPEG / 30 FPS**, a 12-second capture received and decoded **360/360 frames**, averaging **29.96 FPS** including startup. The measured network-source rate was approximately **29.96 FPS**. A separate 235-second continuity capture received and decoded **7,050/7,050 frames** while the sender was deliberately suspended; its independent health task restored it after approximately **169 seconds**. The receiver process and local camera registration were preserved. No image files were saved. This measurement does not establish 4K performance or compatibility with every messenger.

Implementation references: [Microsoft UVC driver](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/usb-video-class-driver-overview), [USB/IP protocol](https://docs.kernel.org/usb/usbip_protocol.html), [USBIP-WIN2](https://github.com/vadimgrn/usbip-win2), [OBS Virtual Camera](https://obsproject.com/kb/virtual-camera-guide), [Unity Capture](https://github.com/schellingb/UnityCapture).
