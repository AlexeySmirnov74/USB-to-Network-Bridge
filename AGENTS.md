# Repository guidance for coding agents

USB to Network Bridge is a Rust USB/IP server and Windows client. The documented target is Windows 11 x64. Read README.md, docs/architecture.md and docs/performance.md before changing capture/transport behavior.

## Project map

- src/vbox.rs, src/native_capture.rs: Native capture through the unchanged signed VBoxUSB driver.
- src/server.rs, src/wire.rs, src/vhci.rs: USB/IP server, protocol and signed USBIP-WIN2 client integration.
- src/desktop.rs, src/server_desktop.rs, src/camera_desktop.rs: Windows interfaces.
- src/uvc.rs, src/video.rs, src/bin/camera.rs: software UVC endpoint and authenticated client-to-server video. Set-VideoMode.ps1 and Restart-VideoEndpoint.ps1 manage its owned tasks.
- src/camera.rs, src/camera_native.cpp: camera capture, controls and frame measurements.
- Root PowerShell helpers implement privileged publication, backend switching, startup and installer maintenance.
- README.md is English; README.fr.md, README.ru.md, README.de.md and README.zh-CN.md carry the same user-facing features.

## Invariants

- Native replaces the userspace server path; it is not a separate newly signed kernel driver. Keep upstream driver signatures intact.
- Sharing one device must not restart unrelated sessions. Reconnect selections follow device identity, not a stale physical port.
- Network video loss must keep the UVC camera registered and supply black without selecting another source. Rename only the verified owned camera, preserving its serial; never physically recapture the generated endpoint.
- Keep transfer buffers and queues bounded and drain asynchronous work before releasing driver handles.
- Selected camera mode and measured received/decoded FPS are distinct; describe screenshots and measurements accurately.
- Keep private configurations, credentials, device inventories, toolchains and logs out of commits and release assets. User-provided screenshots may be included when explicitly requested.
- Document actual behavior and measurements without inventing hardware results or guaranteed performance.

## Verification

For Rust/C++ behavior changes, use Build.ps1 (formatting, Clippy, tests and release build) and relevant hardware checks. Python release/qualification changes use:
    py -3 -m unittest discover -s tests -p 'test_*.py'

For documentation/media changes, validate links, all five README variants and image files; rebuilding or installing drivers is unnecessary.

## Code Review Rules

Flag changes that restart unrelated devices during publication, release driver buffers before completion, expose local credentials in diagnostics/packages, or describe Native as an independent kernel driver. Preserve normal cooperative shutdown and device ownership.

## Versioned updates

The user requires a new version for each published update. Bump Cargo.toml/Cargo.lock, keep GUI/EXE/MSI metadata consistent, describe actual changes in CHANGELOG.md, and publish a new matching GitHub tag and release with tested EXEs, MSI, portable ZIP, source ZIP and hashes. Preserve older tags and releases; do not replace assets in a published previous version.
