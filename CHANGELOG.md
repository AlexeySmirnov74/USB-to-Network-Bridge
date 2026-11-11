# Changelog

## 0.2.4 — 2026-11-11

### Documentation

- Redrawn English architecture diagram shows both physical USB sharing from server to clients and independent client-to-server video through OBS/Unity Capture or a DirectShow camera.
- Diagram identifies the signed capture/controller drivers, configurable default network ports, server-local UVC connection, camera naming, source-loss behavior and network trust requirements.
- All five READMEs use the updated diagram and describe both directions in image alternative text. Application behavior is unchanged.

## 0.2.3 — 2026-11-11

### Added

- English and Russian guides for connecting third-party client applications through OBS/Unity Capture, with GUI/CLI steps, exact source selection, diagnostics and a scoped integration handoff for another project. All five READMEs link to the guide.
- Per-second sender timings for resize, JPEG encoding, socket writes and decoded-callback-to-write completion, along with skipped source frames and bounded buffer usage. These are local measurements, not end-to-end network latency.

### Changed

- Capture wakes on decoded-frame arrival instead of an unconditional 15 ms sleep. Sender BGRA/JPEG buffers are reused instead of allocating new working buffers for each frame.
- JPEG output is bounded during encoding; invalid image dimensions and buffer growth fail cleanly. Reconnection waits for capture shutdown and exits the managed video endpoint if a third-party graph cannot drain, preventing accumulation of stuck capture workers.

### Fixed

- Continuous partial input no longer extends the video read deadline indefinitely.

- Camera renaming now synchronizes the DirectShow capture, Media Foundation camera and video interface names for the exact emulated device. A matching DirectShow name no longer hides a stale Media Foundation label.
- Camera naming readiness waits for both capture APIs and is rechecked after the local USB endpoint reconnects. Physical cameras are rejected before any naming property is written.

## 0.2.2 — 2026-11-10

### Added

- Client-to-server video feeding an emulated USB/UVC MJPEG camera through the unchanged signed USBIP-WIN2 controller and Windows `usbvideo.sys`.
- Configurable camera name and stable serial identity; changing the displayed name updates only the owned camera's DirectShow and PnP properties.
- Server **USB camera — receive video** and client **Send video** dialogs, with dependent controls disabled when the mode is off, and translations in all five languages.
- DirectShow moniker support for software video sources, including installed OBS/Unity Capture filters without a PnP DevicePath.
- Separate network video port, client IP allowlist, mutual connection authentication and per-frame HMAC/replay checks, bounded latest-frame storage, and automatic reconnection.
- Black-frame fallback when a source disappears, preserving the local USB camera registration and leaving physical USB sessions running.
- Video startup tasks and independent one-minute heartbeat checks, recovering an exited endpoint or one whose progress stops for two minutes.
- `usbbridge-camera.exe`, video configuration/status/on/off CLI commands, private connection-key export files, and measured source/output FPS in diagnostics.
- Automatic AVX2 JPEG encoding where supported, with a fallback for other CPUs.

### Fixed

- The generated UVC endpoint is excluded from physical USB publication to prevent recapturing its own output.
- Saving video settings while the selected source is absent preserves its identity instead of replacing it with the test pattern.
- Portable packaging accepts dependency license timestamps older than ZIP's supported date range.
- Video status expires stale connection claims, and setup waits for a newly written ready report rather than accepting an earlier process's file.
- Video mode changes and installer maintenance release the owned virtual USB port before terminating the receiver, avoiding a device enumeration race.
- Installer maintenance stops, resumes and removes the owned video tasks and firewall rules alongside the existing USB components.

## 0.2.1 — 2026-11-10

### Fixed

- A client interface could remain open after its background recovery agent had stopped. Reopening the client now starts supervision for saved device selections.
- Old status files could keep showing a device as connected after the agent stopped. GUI and command-line status now expire stale reports.
- Recovery now waits for a terminated supervisor to release its process lock before starting a replacement.
- Startup recovery could remain stopped after exhausting scheduled restart attempts. Server and client startup tasks now run supervised controllers and have a recurring one-minute trigger.

### Added

- Separate server/client supervisors that monitor control-plane heartbeat progress, restart exited controllers and recover controllers that stop responding.
- Independent one-minute health-check tasks that recover a missing or stalled supervisor, with no retry-count expiry.
- Detection of failed or stalled per-server client reconciliation workers.
- A parent-process watch that ends an orphaned controller while preserving direct kernel USB sessions.
- Structured supervisor logs with child process IDs, exit codes and restart reasons, plus bounded controller stderr/backtrace logs.
- `usbbridge --config FILE server health` and `client health` commands.
- Maintenance disables recurring tasks while updating or intentionally changing server state.

## 0.2.0 — 2026-11-03

- Windows USB/IP server and client with Native Rust and stock server backends.
- Desktop and tray interfaces, command-line control, discovery, reconnect, proxies and camera tests.
- MSI installer, portable package, and separate Server/Client desktop launcher EXEs.
