# Connect a third-party video application on the client

[Русская инструкция](third-party-video.ru.md) · [Video camera quick start](virtual-camera.md)

The producer application, OBS/Unity Capture filter and USB to Network Bridge **video sender run on the client PC**. The receiver on the server creates the USB/UVC camera used by the messenger. This is the reverse direction of physical USB device sharing.

```mermaid
flowchart LR
    A[Client: third-party application] --> B[OBS Virtual Camera or Unity Capture]
    B --> C[USB Bridge video sender]
    C -->|Authenticated TCP/MJPEG, port 13251| D[Server: USB/UVC camera]
    D --> E[Messenger / Windows video application]
```

Start with **1280 × 720 at 30 FPS**. A camera name such as `4k web camera` is independent of its resolution. The receiving machine needs the signed USBIP-WIN2 controller; see the [driver/setup instructions](virtual-camera.md#quick-start). Keep the complete application package and its PowerShell helpers together.

## 1. Provide a DirectShow source on the client

Choose one integration:

| Producer | Setup on the client PC | Source to select in USB Bridge |
| --- | --- | --- |
| Any app whose window or video OBS can capture | Add the application/window/video to an OBS scene, set the output to 1280 × 720 / 30 FPS, and click **Start Virtual Camera** | The installed OBS Virtual Camera source |
| Unity rendering a camera or RenderTexture | Install the x64 Unity Capture DirectShow filter, import its Unity assets and connect the producer camera/texture using the upstream examples | The installed Unity Capture source, normally Unity Video Capture |
| An application that already exposes a DirectShow camera | Start the producer and enable its output | Its exact enumerated camera source |

For OBS, check the Virtual Camera output selection (Program/Scene/Source) so it sends the intended image. See the [official OBS guide](https://obsproject.com/kb/virtual-camera-guide).

For Unity, follow the [official Unity Capture installation and examples](https://github.com/schellingb/UnityCapture): install the filter from a permanent location using `Install/Install.bat`, import its assets and use its camera or `CaptureTexture` example as appropriate. The upstream implementation documents **Direct3D 11** support; verify the producer's graphics API before integrating it rather than changing a working application's settings blindly. Match texture/output dimensions and preserve upstream licenses when redistributing its components. USB Bridge does not install or bundle OBS/Unity Capture.

Run the producer and sender under the **same interactive Windows user/session**. A service or a different remote-desktop session may not share a software camera's image buffers. Close extra viewers if the source only permits one capture consumer. A selected source is never replaced with another camera when it disappears.

## 2. Configure the server

1. Open **Server → USB camera — receive video** and enable reception.
2. Set the desired name, for example `4k web camera`, and choose 1280 × 720.
3. Keep incoming TCP **13251**, or select a free port. Allow the client's actual LAN/VPN IP in server settings.
4. Enter a shared connection key of 16–128 characters. Save the settings.

The video port is independent of physical USB TCP 3240 and discovery 3241. TCP **13250** is the receiver's local-only USB/IP connection; the producer does not connect to that port. Authentication protects integrity and access, **not image confidentiality**: use a trusted LAN or VPN.

## 3. Configure the client

1. Open **Device → Send video**, enable it and select the producer's source.
2. Enter the server's reachable `IP:13251`, or its configured video port.
3. Enter the same key and apply. A blank key field preserves the existing saved key.
4. On the server, select the configured USB camera in Camera Test or the messenger. Enable Windows camera permissions if requested.

The sender runs at user logon; the receiver runs at startup. Independent scheduled health checks recover a stopped or stalled endpoint while the mode is enabled. Turning the mode off disables its recovery tasks. Do not leave a diagnostic viewer holding an exclusive source when starting the sender.

## Command-line integration without changing the key or destination

After the first GUI setup, enumerate the available client sources:

```powershell
Set-Location C:\USBBridge
.\usbbridge.exe --config .\client.local.json client cameras
.\usbbridge.exe --config .\client.local.json client video settings
.\usbbridge.exe --config .\client.local.json client video status
```

Use the exact `path` (or an unambiguous USB identity) returned by `client cameras`. Names can be duplicated; do not automatically select the first camera. Older Bridge versions could expose Unity Capture's literal `foo:bar` registration placeholder. Version 0.2.5 uses each slot's unique DirectShow moniker instead. Keep the exact enumerated path; do not copy the placeholder into a new configuration. For separate client and server sources, see the [dual-camera setup](dual-camera.md).

This example updates an **already configured** sender. It preserves `server` and `key_file` and never reads or prints the key. Replace the source placeholder with an enumerated path:

```powershell
$runtime = 'C:\USBBridge'
$config = Join-Path $runtime 'client.local.json'
$settings = Join-Path $runtime 'client.video-send.local.json'
$request = Join-Path $runtime ('video-request-' + [guid]::NewGuid().ToString('N') + '.local.json')
$sender = Get-Content -LiteralPath $settings -Raw -Encoding UTF8 | ConvertFrom-Json
$sender.enabled = $true
$sender.camera = '<EXACT_PATH_FROM_CLIENT_CAMERAS>'
$sender.quality = 85
try {
    [IO.File]::WriteAllText($request, ($sender | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
    & (Join-Path $runtime 'usbbridge.exe') --config $config client video configure $request
    if ($LASTEXITCODE -ne 0) { throw 'Video configuration failed' }
} finally {
    Remove-Item -LiteralPath $request -Force -ErrorAction SilentlyContinue
}
```

`video configure` can request administrator rights to update owned scheduled tasks. Keep configurations/keys private. DPAPI `.credential` files are bound to their originating Windows account/machine; copying one is not a way to transfer a key. Use the GUI on each PC or the private-key export workflow in the [video guide](virtual-camera.md#command-line).

The current sender accepts **enumerated DirectShow cameras or `test-pattern`**. It does not accept an RTSP/RTMP URL, a raw FFmpeg pipe, or arbitrary JPEGs on TCP 13251. That port uses a versioned authenticated protocol. To add a direct producer API, implement/document a separate adapter; do not bypass the authentication handshake.

## Verify the complete path

On the server run:

```powershell
.\usbbridge.exe --config .\host.local.json server video status
```

Check `fresh`, `running`, `device_attached`, `name_applied` and `source_connected`. `received_frames` should increase and `source_fps` should follow the producer. `camera_fps` measures delivery to an application with the camera open and can be zero without a consumer. Sender status includes per-second timing in microseconds for resize, JPEG encoding, socket writes and decoded-callback-to-write completion, plus skipped source frames and buffer sizes. These are **local processing measurements**, not exposure-to-screen or one-way network latency.

Use a visible changing frame counter or animation from the producer to verify content. A 30 FPS black fallback or a software filter repeating its last image does not prove that new producer frames are arriving. If video is absent, first verify the producer locally, then source identity/session, sender status, receiver allowlist/port/key, and finally camera permissions in the receiving app.

If the network/source stops, the server keeps its camera registered and switches to black after two seconds without a fresh frame. The sender reconnects automatically. Some third-party filters continue supplying a frozen image or their own error pattern even when the producer stops; test their behavior separately.

Version 0.2.3 synchronizes names across DirectShow and Media Foundation interfaces for the owned camera. Apps can cache a former name: refresh their camera list or reopen the app after renaming. The camera's serial remains unchanged. Its virtual USB controller remains visible in Device Manager.

## Handoff to an assistant working on the producer project

Give that assistant this repository and the following scoped task, with the real paths supplied privately:

> Integrate this Windows video producer with USB to Network Bridge on the client PC. Read `docs/third-party-video.md`. Inspect the existing OBS/Unity Capture setup and the exact sources from `usbbridge client cameras`; preserve the selected server, key file, physical USB choices and other project settings. Produce 1280 × 720 at 30 FPS with a changing counter, enable the selected source in the existing video sender, and verify increasing received frames and the configured camera name on the server. Do not print keys, modify driver signatures, substitute an unrelated camera, or claim measured end-to-end latency from local timing counters. Report the tested source, format, FPS, recovery and any remaining limitations.
