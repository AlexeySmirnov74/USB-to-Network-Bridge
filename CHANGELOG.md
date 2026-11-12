# Changelog

## 0.2.13

- Add a Russian architecture diagram and replace resolution-like example camera names in both diagrams with descriptive virtual USB camera labels.

- Refresh the English architecture diagram for both video destinations, silent USB microphones and offline deployment.

- Include the missing pinned GlobalSign issuing CA certificate for offline client-installer verification on a clean Windows PC.
- Cache only that non-root intermediate under the current user; retain Windows root trust and full cache-only Authenticode verification of the unchanged installer.
- Add read-only, network-free developer diagnostics for driver certificate chains.

## 0.2.12

- Normalize Windows short and long path aliases before first-run backups so a copied key is archived once on a new PC.
- Isolate watchdog tests from prebuilt runtime files and verify explicit-exit holds without starting real tasks.
- Run push CI once on the main branch instead of duplicating the same build for its release tag.

## 0.2.11

- Recognize a correctly staged VBoxUSB package on a fresh server before any device has been captured; avoid repeated repair/reboot prompts.
- Bundle the unchanged production-signed usbipd-win and USBIP-WIN2 installers in the portable ZIP and MSI for offline first-run setup and repair.
- Require local SHA-256, exact publisher and cache-only Windows Authenticode verification; missing or corrupt installers produce a local error instead of a network request.
- Install USBIP-WIN2's embedded Visual C++ runtime explicitly and ensure the local receiver driver before enabling a generated USB camera.
- Verify the complete offline payload during packaging and folder relocation; include upstream licenses and matching driver-source archives in the release.

## 0.2.10

- Remove the fixed five-second full-exit grace period for video and other workers that cannot observe it.
- React to an explicit exit during supervisor waits and hand shutdown to the coordinator after USB operations drain; block delayed helpers from restarting an exited backend.
- Use the direct Task Scheduler API with fresh ownership checks and stop only running owned tasks, preserving saved startup preferences.
- Launch the privileged lifecycle helper directly through normal Windows elevation and log shutdown phase durations.

## 0.2.9

- Add File and tray menu commands to fully exit each role, release its USB connections and stop its owned video/background processes.
- Honor explicit exit in controllers, supervisors and health checks until manual reopening or the next Windows boot, while preserving startup preferences.
- Add matching command-line `quit` and restore owned enabled tasks when reopening an exited application.

## 0.2.8

- Store configuration file paths relative to their owning JSON; resolve the application directory with `@app` independently of the working directory.
- Preserve identities and credentials during same-PC folder moves, and rebind owned Windows startup tasks, firewall rules and shortcuts on launch.
- Keep legacy absolute settings readable and migrate active settings without resetting server/device selections.

## 0.2.7

- Fix false server-stopped status in a limited GUI when the Native backend runs under SYSTEM; allow only metadata queries on the verified backend process.
- Keep only the Client and Server launchers at the portable folder root; store runtime files under `app/`.
- Detect settings copied from another Windows installation. Create safe new server defaults and let clients keep server connections or start fresh.
- Check signed driver readiness at startup and run the pinned upstream installers through normal UAC when drivers are missing; never restart Windows automatically.
- Migrate owned startup, recovery and video tasks to the new layout while preserving camera identities and existing connections.

## 0.2.6

- Add an optional silent USB Audio Class microphone to each generated USB/UVC camera.
- Camera and capture audio share one composite USB parent and device container; microphone data is always zero PCM, with no physical microphone capture or network audio.
- Add a saved Silent microphone setting to server and client-local camera dialogs; old configurations remain video-only until enabled.
- Keep USB identities, video format and independent routing while re-enumerating only the configured generated camera.

## 0.2.5

- Add a separate, renameable USB/UVC camera on the client and a loopback-only receive mode.
- Keep client-local and network video senders independent, with separate scheduled recovery tasks and keys.
- Identify Unity Capture slots by their unique DirectShow monikers, avoiding duplicate placeholder identities.
- Add explicit Unity Capture Server / Client producer setup and reject generated USB camera capture loops.
- Resolve interactive capture accounts from the Windows token and grant private keys to the creating user, including elevated setup followed by a limited sender.
- Allow installer maintenance to resume owned video tasks as well as controller tasks.

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
