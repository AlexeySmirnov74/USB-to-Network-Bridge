# Measuring performance

Use the same physical device, USB port, firmware, network path, video format, driver versions and test duration for both capture modules. Stop the camera application and detach the client before switching. Reattach after the switch and wait for Windows device interfaces to appear.

The diagnostics count frames and compressed payload bytes in memory. They do not save images. `Build-CameraProbe.ps1` builds DirectShow and Media Foundation probes with MSVC/Windows SDK. For example, on the client:

```powershell
.\camera-dshow.exe 360 1280 720 "2K FHD camera"
```

Arguments are duration in seconds, width, height and exact friendly name. The DirectShow probe selects MJPEG at 25–30 fps. Its own watchdog bounds a diagnostic run; this timer is not present in the server/client. The Media Foundation probe currently targets the hardware fixture VID:PID `1bcf:0b40` and selects up to 1080p.

Report frames, elapsed time, payload MiB/s, CPU usage, device resets and reconnects. Camera payload depends on the scene and its compression. USB/IP payload rates therefore should not be treated as a line-rate network benchmark. A camera output of 30 fps cannot demonstrate that one module is faster than another that also delivers 30 fps.

`Test-Transport.ps1` starts a localhost synthetic device and tests pipelined IN/OUT traffic. Or run the release binaries directly:

```powershell
.\usbbridge-server.exe --mock --export all --listen 127.0.0.1:13240
.\usbbridge-client.exe bench 127.0.0.1:13240 256 262144 32 out
.\usbbridge-client.exe bench 127.0.0.1:13240 256 262144 32 in
```

These exercise the Rust protocol and TCP path, not a USB controller or physical device. Previously observed localhost figures and SSH-tunnel throughput must not be used to claim real USB throughput or a performance advantage over usbipd-win.

For storage, use a dedicated disposable test volume and verify data integrity in addition to throughput. Storage and mixed-device stress tests have not yet been performed. No such test is run automatically on a user's existing disks.

## Chicony B729, 1280×720 MJPEG (2026-11-03)

An operator reported approximately 30 FPS locally and 24 FPS after USB/IP import.
These short, sequential measurements used the same camera and 30 FPS media type:

| Capture path | Duration | Received / decoded frames | Average received FPS |
| --- | ---: | ---: | ---: |
| Local Windows KSProxy | 12.006 s | 356 / 356 | 29.65 |
| Native, normal client receive mode | 12.026 s | 270 / 270 | 22.45 |
| Native, event-based client receive mode | 10.023 s | 223 / 223 | 22.25 |
| usbipd-win 5.3.0, normal receive mode | 12.010 s | 277 / 277 | 23.06 |
| Native, suggested capture buffer count 8 | 10.040 s | 230 / 229 | 22.91 |
| Native, driver speed initialized before claim | 15.026 s | 446 / 446 | **29.68** |

Startup is included in these averages. They are short diagnostic comparisons,
not a controlled benchmark establishing a performance difference between modules.
The client's normal receive mode and the server's Native selection were restored.
Only Chicony was imported during these measurements.

The server used Wi-Fi with an 865 Mbps negotiated link; the client used 1 Gbps
Ethernet. Thirty ICMP measurements were 1–2 ms (mean 1.07 ms). Compressed video
payload was approximately 8–10 Mbps. Link rate and ping alone do not prove that
wireless jitter or driver scheduling cannot affect isochronous delivery.

Locally, sample timestamps and arrival times progressed approximately every 33 ms.
Both USB/IP server modules delivered groups of frames with approximately 128 ms
gaps, followed by frame arrivals a few milliseconds apart. Native logged 128-packet
isochronous requests of 344064 bytes each. Received and decoded totals agreed, so
the observed shortfall preceded decoding and window painting. The actual capture
allocator had ten 1843200-byte buffers; requesting more buffers did not restore
30 FPS. The optional dropped-frame interface did not provide reliable per-USB
packet loss accounting and its zero counters are not proof of zero losses.

Further Native instrumentation found a full queue of sixteen ISO requests and no
USB packet-status errors, but the camera's UVC payload headers flagged errors.
An endpoint with `bInterval=1` and High-Speed should transfer eight packets per
millisecond. Blocks of 128 packets instead completed every 128 ms, with roughly
two seconds of requests queued in the capture driver.

The missing operation was VBoxUSB's `GET_DEVICE` ioctl. In the
[upstream driver](https://github.com/mirror/vbox/blob/master/src/VBox/HostDrivers/VBoxUSB/win/dev/VBoxUsbRt.cpp),
`vboxUsbRtDispatchGetDevice` initializes the cached High-Speed flag used by
`vboxUsbRtUrbSend`. Without that initialization, this USB 2 High-Speed camera was
scheduled with full-speed timing. Native now performs the query before claiming
each device and fails initialization if that query fails. Tests enforce this
ordering and verify that failed speed initialization cannot proceed to a claim.
The installed production-signed driver is unchanged.

With this correction, the same imported Chicony delivered and decoded 446 frames
in 15.026 seconds: **29.68 FPS**, including startup, compared with 29.65 FPS in
the short local baseline. The USB completion interval fell to approximately
16 ms and the observed steady-state UVC error flags disappeared. Sample arrival
deltas returned to approximately 33 ms instead of 128 ms bursts. This establishes
recovery of the reported 30 FPS mode in this fixture; it is not a claim about all
cameras, networks or sustained performance under load. The stock backend was not
modified by this Native initialization fix.

An interactive GUI run on the installed client then received 911 additional
frames in 30.319 seconds after warmup (**30.05 received FPS**, **30.08 decoded
FPS**). The decoded delta includes one sample already waiting for decoding at
the first snapshot. Per-second GUI readings remained around 30 FPS. The preview
closed normally and the USB settings file remained byte-for-byte unchanged.

The transport already uses a full-duplex TCP connection with independent receive
and send threads, pipelined USB requests and `TCP_NODELAY`. An additional TCP port
would not correct this scheduling error. Continuous queues and the endpoint's
actual interval matter for time-sensitive USB transfers; see the
[Linux USB queueing documentation](https://www.kernel.org/doc/html/latest/driver-api/usb/usb.html)
and [Microsoft's ISO transfer guidance](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/transfer-data-to-isochronous-endpoints).
The bounded debug trace records queue depth, timing, USB packet errors and UVC
header counters without saving camera images or payloads.

## Low-light compensation and a separate 15 FPS case (2026-11-03)

A later report reproduced approximately 15 FPS **locally on the server**, before
USB/IP was involved. Chicony still advertised 1280×720 MJPEG at 30 FPS, but delivered
approximately 66.8 ms sample intervals. Its auto-exposure-priority control was 1.
Temporarily setting it to 0 produced 296 frames in 10.004 seconds (29.59 FPS).
This was distinct from the earlier Native High-Speed initialization correction.

Camera Test now has a live **Low-light compensation (may reduce FPS)** checkbox.
Off is the default; it asks the camera to retain the selected frame rate rather
than lengthen exposure by reducing the rate. It does not invent additional frames
or increase the camera's advertised maximum. On allows the camera's adaptive
low-light policy. The original hardware property and its flags are restored at
normal stop/close, and a camera without the control is handled without failing
video capture.

The observed received-frame rates were:

| Camera / side | Off | On | Off again |
| --- | ---: | ---: | ---: |
| Chicony, actual server GUI | 30.16 | 15.12 | 30.12 |
| Chicony, actual client GUI | 30.15 | 14.99 | 30.02 |
| 2K FHD, actual client GUI | 29.97 | 16.63 | 29.80 |

All rows used 1280×720 MJPEG with a 333333 × 100 ns interval. The server-GUI
intervals were delimited by actual control-change events, because additional
manual clicks occurred during its automated sampling. They span approximately
7.06, 5.03 and 2.03 seconds respectively. Client intervals span approximately
6.06–6.07 seconds per state. Decoded counts followed received counts; an occasional
one-frame sample-boundary difference is expected in the asynchronous graph.
The client tests preserved USB configuration and restored the exact original
low-light property/flags. No camera images were saved. These are short measurements
in the reported lighting conditions, not a guarantee of a particular rate in
every scene or supported resolution.

Microsoft documents the effect of this control in
[auto-exposure priority](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority).

## Client-to-server software video path (v0.2.3, 2026-11-11)

The DirectShow capture worker and video encoding/socket worker already run on
separate threads. More queued frames would age the image under load, so the
bridge retains the newest decoded frame rather than an unbounded FIFO. JPEG
encoding already selects AVX2 on supported CPUs. The new capture wakeup removes
an unconditional 15 ms sleep; native waiting wakes on a decoded sample and is
bounded so controls remain responsive.

The sender now reuses its output BGRA/JPEG buffers. JPEG growth is checked while
writing, up to the receiver's announced frame limit (at most 16 MiB); dimension
arithmetic and capture frames are bounded separately. A failed reconnect drains
the old capture worker before opening another. If a third-party graph does not
shut down in five seconds, the managed video endpoint exits for its existing
recovery task instead of accumulating further capture workers.

A live Unity Capture source was kept unchanged on the Windows client. With the
receiver set to 1280 × 720 / 30 FPS, 24 process/status samples over 46.35 seconds
showed approximately **29.88 sender FPS**. The generated server camera separately
received and decoded **360/360 frames in 12.021 seconds (29.95 FPS)**.

| Sender stage | Mean across the sampled status windows |
| --- | ---: |
| Resize/orientation | 1.33 ms |
| JPEG encoding | 8.89 ms |
| Socket write, including frame authentication | 0.48 ms |
| Total processing | 10.72 ms |
| Native decoded-frame availability to completed socket write | 11.95 ms |

These counters use the sender's monotonic clock. A completed socket write does
not mean that the receiving application has displayed the frame. The values do
not include producer rendering/exposure, full network transit, USB consumption
or messenger rendering; no end-to-end latency reduction was measured.

CPU usage was approximately **51% of one logical CPU** both before and after;
the short comparison does not establish a CPU speedup. Process private bytes
ranged from **19.80 to 23.52 MiB**, changing by −20 KiB between the first/last
samples. Handles stayed at 234–235 and threads at 11. The reused 720p BGRA buffer
was 3,686,400 bytes and JPEG capacity stayed at 131,072 bytes. This sampled run
showed no progressive growth; it is not proof that every external filter or
long-lived workload is leak-free. See the [memory audit](memory-audit.md).
