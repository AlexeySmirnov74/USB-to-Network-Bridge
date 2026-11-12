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

Respect the user's request for focused verification: run formatting, Clippy, a release build, and tests of changed scenarios with relevant hardware checks. Reuse accepted results for unchanged code; do not repeat the full suite or GPU/long runs by habit. Build.ps1 remains the full verification entry point when a full run is actually justified. Python release/qualification changes use:
    py -3 -m unittest discover -s tests -p 'test_*.py'

For documentation/media changes, validate links, all five README variants and image files; rebuilding or installing drivers is unnecessary.

## Code Review Rules

Flag changes that restart unrelated devices during publication, release driver buffers before completion, expose local credentials in diagnostics/packages, or describe Native as an independent kernel driver. Preserve normal cooperative shutdown and device ownership.

## Versioned updates

The user requires a new version for each published update. Bump Cargo.toml/Cargo.lock, keep GUI/EXE/MSI metadata consistent, describe actual changes in CHANGELOG.md, and publish a new matching GitHub tag and release with tested EXEs, MSI, portable ZIP, source ZIP and hashes. Preserve older tags and releases; do not replace assets in a published previous version.

## Camera checks

Camera name checks must compare fresh DirectShow and Media Foundation enumeration, including their separate interface properties. For video changes, preserve the configured producer and keys, measure actual frame progress and bounded buffers, and distinguish local processing timings from end-to-end latency. See docs/third-party-video.md for integration with another project.

## Dual video destinations

Read docs/dual-camera.md. Client local-camera settings create a separate loopback-only receiver and LocalSend task, keeping the original network sender unchanged. Unity Capture Server and Client must resolve to unique exact monikers (slots 0/1); the upstream foo:bar placeholder is not unique. Keep UVC serials stable across rename and reject generated VID1209/PIDF002 sources. Private key ACLs must explicitly grant TokenUser, SYSTEM and Administrators; OWNER RIGHTS alone is insufficient for elevated setup followed by a limited sender. Task Scheduler capture principals use the real Windows token SAM account, not USERDOMAIN hints.

## Optional silent USB microphone

`camera.microphone` defaults false. Enabled receivers expose UVC video and a UAC1 microphone in the same USB composite container, preserving the camera serial. The microphone generates only local 48 kHz mono PCM16 zero samples; never open a physical microphone, forward audio, or select a default audio source. The inbox Windows audio driver and existing signed virtual controller remain unchanged. Audio Endpoint Builder and Windows Audio must run for Windows to expose the capture endpoint. Applications may still require a separate microphone selection.

Keep audio work independent of video, bound ISO queues/packet counts, preserve original ISO offsets when compacting USB/IP replies, and coordinate completion handoff with unlink/alternate shutdown. Audio pacing uses an absolute deadline: do not reset it for ordinary late Windows timer wakeups, which otherwise slow audio to roughly 64% of the declared rate. Only genuine idle or bounded excessive lag rebases the clock.

For changes here use the focused silent_microphone tests and bounded capture of the verified generated microphone/container. Check zero samples, sustained frame delivery, and video progress; never record or probe a physical/default microphone. Preserve unrelated transport processes and device selections.

## Offline deployment

The release MSI and portable ZIP must contain both exact, production-signed installers from `driver-installers.json` under `app/tools/driver-installers`. Setup, repair and receive-mode prerequisites use local-only verification/installation; never add a runtime HTTP fallback or certificate/revocation fetch. Only the explicit build-time `Download-Drivers.ps1` may fetch those files. Preserve pinned SHA-256 and exact signer validation, include upstream notices, and publish matching corresponding-source archives alongside redistributed driver installers. Permit installer binaries only at their exact manifest-checked paths; keep all other runtime/private binary exclusions.
