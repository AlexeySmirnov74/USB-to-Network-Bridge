# Changelog

## 0.2.1 — 2026-11-10

### Fixed

- A client interface could remain open after its background recovery agent had stopped. Reopening the client now starts supervision for saved device selections.
- Old status files could keep showing a device as connected after the agent stopped. GUI and command-line status now expire stale reports.
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
