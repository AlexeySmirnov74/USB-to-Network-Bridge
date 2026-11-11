# Memory and resource audit — 2026-11-03

The audit covered the Rust server/client, capture backends, proxy relay, native
Windows GUI and the C++ camera bridge. Confirmed accumulation paths were fixed;
additional failure cleanup and memory accounting were tightened. Tests count
live Rust allocations, kernel handles and GDI/USER objects. Working-set size
alone was not used to infer a leak.

## Findings and corrections

| Area | Finding | Correction |
| --- | --- | --- |
| Settings dialogs | Windows destroyed a closed dialog's menu, but its owner-draw strings remained in the process-wide label map on each open/close. | Detach and retire menu metadata before destroying the window. Also destroy a live modal window before dropping its Rust context when its message loop fails or consumes `WM_QUIT`; propagate the quit message. |
| Camera worker | Recreating the last COM MTA on the capture thread retained approximately 17 handles per camera lifecycle on the tested Windows/KSProxy path. Mode enumeration alone reproduced it. | Keep a single MTA initialized for the preview worker's lifetime; release its camera session before balancing that initialization on the same thread. |
| Native on-demand capture | Repeated failed acquisitions could append a new tokenized restoration request while earlier cleanup for the same device remained busy or failed. | Reserve each identity through capture and pending/in-flight cleanup. Further acquisitions return `WouldBlock`; successful restoration releases the reservation. |
| USB transfer accounting | UsbDk reserved payload bytes but omitted its ISO descriptors/results/lengths. Zero-payload ISO requests could therefore consume substantial memory outside the quota. Native's descriptor accounting was also incomplete. | Include request structures and ISO metadata in the byte lease. Account for UsbDk rejected-reply metadata as well. Existing quota leases remain held until outgoing completions are released. |
| UsbDk synchronous IOCP completion | If posting a synthetic completion failed, no kernel packet would subsequently retire that request. Its pinned buffers could remain pending and prevent shutdown. | Finish that already-completed operation inline, removing its pending entry and releasing its lease through the normal completion path. |
| CLI benchmark | Its sender retained a spare credit-channel sender. A failed receiver could leave the producer waiting forever for another credit, retaining the connection and buffers. | Give the receive thread the last credit sender, so failure wakes the producer and tears down the session. |
| Proxy shutdown | A connection could be registered after the shutdown socket snapshot. Worker-creation failure also left a registry entry behind. | Stop/join the listener before closing the registered sockets; remove failed-worker registrations and skip new proxy handshakes when stopping. |

The camera bridge additionally uses RAII for property `VARIANT` values and
SetupAPI device-information lists, including exception paths. Owning media-type
objects cannot be copied. Failed owner-draw menu registration removes the newly
allocated label instead of retaining it.

The capture thread follows Microsoft's requirements for
[per-thread COM initialization](https://learn.microsoft.com/en-us/windows/win32/learnwin32/initializing-the-com-library)
and [balanced uninitialization](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-couninitialize).
The experiment identifies the MTA lifecycle that triggers handle retention; it
does not identify the internal Windows component owning each retained handle.

## Automated verification

- The modal-window regression closes 32 real settings forms, then quits with a
  33rd form open. Its menu-label table returns to baseline after each cycle and
  the quit message is preserved.
- A failed-acquisition fixture makes 1,000 further attempts for the same identity
  while cleanup is pending: exactly one release request remains queued. Busy
  cleanup is retried and a successful release frees the identity reservation.
- ISO metadata is charged even for a zero-byte payload; the quota test releases
  every lease and verifies a zero balance.
- The Windows IOCP fixture covers normal transfers, real overlapped cancellation
  and failed synthetic posting. Pending storage and byte leases are released.
- A malformed benchmark response wakes a producer that exhausted its only credit.
- Dropping a proxy relay closes all 16 active local sockets and their proxy-side
  counterparts. Existing HTTP, HTTPS and SOCKS5 tests also pass.
- `tests/memory.rs` tracks live Rust allocations through 24 warmup sessions and
  **200 subsequent complete bulk sessions / 200 MiB** over loopback. Every server
  connection fully retires before measurement. In the recorded run, retained
  Rust bytes changed by **−120 bytes**, and process handles fell from **93 to 89**.
  The final server has no remaining connection owners. These are synthetic USB
  protocol tests, not physical-device throughput measurements.

Formatting, Clippy with warnings denied and **92 automated tests** passed
(64 unit, 4 console, 17 protocol, 6 proxy, 1 memory lifecycle).

## Real camera lifecycle measurements

The Chicony B729 was tested at 1280×720 MJPEG. No camera images or USB payloads
were saved. USB publication and remembered device choices were preserved.

| Test | Observed result |
| --- | --- |
| Previous per-operation MTA lifecycle, 20 captures | Handles increased from **256 to 579**, exactly 17 per further cycle. |
| Mode enumeration alone, 20 operations | Handles increased from **240 to 563** and remained there after 15 idle seconds. |
| Persistent MTA, 20 captures | Handles stayed around **334–336**, including a 15-second idle observation. Private bytes varied between approximately **3.14 and 3.90 MB**. |
| Installed Rust GUI, 20 start/stop cycles in one preview process | From cycle 5 to 20: **+2 handles, +0 GDI, +0 USER**, private bytes **+241,664 bytes**. The measured private-byte values fluctuated between **4.42 and 5.35 MB**, rather than increasing each cycle. The process closed normally. |

The installed GUI test used one persistent preview process throughout; closing
and recreating the process between samples would have hidden a resource leak.
Both imported cameras remained connected during that test. Updated server/client
applications were installed on the notebook and remote client. Updating the Native
server required one transport restart with automatic USB recovery; subsequent
GUI updates and camera cycles preserved the new transport sessions.

## Limits of the result

This was a source audit plus short, repeatable lifecycle tests. It is not a
24-hour soak test, an exhaustive heap trace of Windows multimedia components,
or a kernel-pool audit of the upstream production-signed drivers. Small leaks
below the sampling resolution, hardware-specific error paths and long-lived
workloads still need longer testing. The memory test allows 256 KiB of retained
Rust allocations for test-runner output/lazy caches; it detects retained transfer
buffers per session, not every possible single-object leak.

The two shared palette brushes and the reusable preview bitmap are intentional
bounded caches. Camera storage keeps the latest frame rather than an unbounded
video queue. Resource growth should therefore be compared after warmup, at the
same capture state, and across enough cycles to distinguish caching from a trend.

## Software video follow-up — v0.2.3, 2026-11-11

The client-to-server video path was reviewed for per-frame allocation, oversized
input, lifetime across reconnects and persistent queues. It retains one latest
decoded frame; consumers hold an `Arc` while processing it. Cloning that handle
does not clone its pixel storage. Network ingress admits at most four handshake
workers and one authenticated active sender. Its complete JPEG slot and the
current UVC frame are bounded; camera frames are capped at 64 MiB and network
JPEGs at the announced mode limit, at most 16 MiB. These are per-buffer bounds,
not a 16 MiB limit for the entire application process.

Changes reuse the sender's BGRA/JPEG working storage, enforce JPEG output growth
inside the writer, reject invalid/overflowing output dimensions and enforce a
read deadline even while bytes continue to trickle in. Before reconnecting, the
sender waits for its capture worker to finish. If an external DirectShow graph
cannot drain, only that managed video process exits for recovery, avoiding an
unbounded series of abandoned capture threads. No physical USB session is
restarted for this video cleanup.

Regression tests encode repeated frames without reallocating the warmed output,
reject a JPEG exceeding a deliberately small writer bound, check orientation
and letterbox reuse, reject overflowing dimensions and time out continuous
partial input. The existing transfer lifecycle/memory tests also passed.
`Build.ps1` passed 95 unit and 29 integration tests; 23 Python qualification tests
passed separately.

On the unchanged live Unity Capture source, 24 samples over 46.35 seconds showed
19.80–23.52 MiB private bytes, −20 KiB first-to-last, 234–235 handles and 11
threads. The sender remained connected throughout. The BGRA working buffer was
3,686,400 bytes and JPEG capacity 131,072 bytes. This is evidence of bounded
storage and no progressive growth in the sampled workload, not an exhaustive
heap/kernel-pool audit or a guarantee about every third-party filter. Timing
and FPS are recorded in [performance measurements](performance.md).
