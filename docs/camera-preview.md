# Camera preview and FPS

Both Windows interfaces have a **Camera Test** button and menu action. The client
also offers it in its device context menu. The server offers it in the device list
context menu. **Right-click a camera → Camera Test** selects its exact USB identity
and starts it immediately when available locally. Requesting a different camera
switches the existing preview to it; requesting the current camera keeps it running.

1. Open **Camera Test**.
2. Choose the camera and a video mode.
3. Click **Start preview**. The general button/menu opens an idle chooser; the
   per-device context action starts the chosen camera automatically.
4. Click **Stop preview** or close the preview window to release the camera.

On both roles, the test window is modal: the main application window is disabled until
the test closes. Background USB transport, discovery and recovery continue. Closing
the test (including termination of its process) releases the main window again.

**Low-light compensation (may reduce FPS)** is a manual checkbox on both roles.
Without a saved client preference it defaults to off so the camera can maintain the selected frame rate. Enable it
to allow longer exposure and a brighter low-light image, at the cost of a lower
frame rate when the camera requires it. The checkbox can be changed while preview
is running; no USB detach or network reconnection is needed. Unsupported cameras
have a disabled checkbox once their control support has been queried. The original
hardware setting is restored when preview stops or closes normally. The setting
affects Camera Test, rather than saving a permanent Windows camera preference.

This controls Windows' `KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY`; see
[Microsoft's description](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority).
Received and decoded frame counts measure actual frames, independently of the
advertised mode's FPS and of window repaints. Control changes and restoration are
recorded without saving camera images.

The default prefers a supported MJPEG mode near **1280×720 at 30 FPS**. All listed
modes come from the camera's Windows capture driver; changing resolution or frame
rate requires stopping the preview first. The window preserves the image aspect
ratio when resized. Each image is composed off screen and presented in a single
buffered operation to avoid a black flash between frames. The controls and messages support all five interface languages.
**Help → About** includes the project repository, and **Language** changes the saved
language for that role without restarting the camera capture.

## Which side can show the camera

The viewer opens a camera available in **Windows on the same PC**:

- On the client, connect/import the USB camera first. It then appears as a normal
  Windows video capture device.
- On the server, a camera captured for USB sharing is unavailable to local video
  applications. For a local test, stop sharing it and refresh the camera list. With
  the per-device local-use preference enabled, an idle published camera can also
  be available locally. An active client owns the camera exclusively.

Preview never changes publication, takes a device away from a client, switches the
capture backend, or detaches a virtual USB port. Stopping preview releases only the
video application; the USB connection stays available for another application.

The camera's device path and USB parent identity select it; its display name alone
does not. If a requested camera is absent or ambiguous, the viewer does not switch
to another camera or automatically start one later selected by hand. Reopening the
test for another device focuses the existing preview window, stops the old local
preview, and starts the explicitly requested camera.

If the camera is unplugged or its driver reports a capture failure, the preview
shows an error and clears the old image. Refresh and start it again after Windows
has restored the device. If frames stall without a device removal, current FPS
falls to zero and a warning appears; the existing stream can resume when frames
return. If a newly started capture produces no input samples for five seconds, the
viewer retries the same camera and video mode once. This releases/reopens only the
local video graph, without detaching USB or changing publication. The retry and
its camera/mode/statistics are logged as `camera.startup.retry`. Persistent failure
stays visible and does not loop through cameras or restart indefinitely. Preview
has no automatic session time limit.

## Reading the numbers

- **Received FPS** counts complete video samples arriving from the Windows camera
  source over elapsed monotonic time. It is not the configured FPS or a repaint
  timer. Current rates update about once per second and can fluctuate with buffering.
- **Decoded FPS** counts samples converted to RGB32 for the preview. Rendering uses
  the latest available frame, so a slow or obscured window does not accumulate a
  backlog of old pictures. This value describes decoding, not monitor refresh rate.
- **Average FPS** includes startup and any stalls since Start. Short tests can have
  a lower average while current FPS has already reached its steady rate.
- **Video MiB/s** counts camera sample payload bytes before decoding. It excludes
  USB/IP, TCP, proxy and Ethernet overhead and is not a network throughput benchmark.

The separate preview process isolates third-party capture filters from the USB
transport and main interface. Closing it first requests normal graph shutdown; a
blocked capture filter cannot hold the main interface open. Frame buffers contain
only the newest preview images in memory and are bounded to 64 MiB per native frame.
Images are not recorded or included in development logs.

**Camera permissions** opens Windows' camera privacy settings when clicked. It does
not change permissions. A camera may also be busy in another video application;
the error includes the DirectShow HRESULT for diagnosis.

## Command line

```powershell
.\usbbridge.exe client cameras
.\usbbridge.exe client camera-modes 'USB\VID_1BCF&PID_0B40\CAMERA_SERIAL'
.\usbbridge.exe client preview
.\usbbridge.exe client preview 'USB\VID_1BCF&PID_0B40\CAMERA_SERIAL'
.\usbbridge.exe client camera-test 'USB\VID_1BCF&PID_0B40\CAMERA_SERIAL' --seconds 10
```

The same commands are available under `server`. Camera selectors accept the exact
device path returned by `cameras`, its own PnP identity, or its USB device identity.
`camera-modes` returns mode indices; use `--mode N` to select one in `camera-test`.
The benchmark defaults to 10 seconds, accepts 1–600 seconds, emits JSON statistics
each second, and saves no images. `preview` opens a window in the current interactive
Windows session. With a selector it selects and starts that exact camera; without
one it opens the idle chooser. Its lifetime is independent of the benchmark duration.

Development logs are `logs/server-camera.debug.jsonl` and
`logs/client-camera.debug.jsonl`. They include enumeration, identities, supported
modes, selected format, button actions, sample counts, FPS timing, errors, painted
frame counts and cleanup results. The normal CLI log contains benchmark statistics.

On the **client**, Low-light compensation is saved automatically for the selected
camera in `client.local.camera.preferences.json` beside the active configuration
(the filename follows the config name). The complete USB parent identity, including
the serial/instance component, is used; identical model names or VID/PID values do
not share a preference. With no saved value it defaults to off. A change during
preview is saved only after the hardware accepts it; an idle change saves the next
preview's requested value. A corrupt preference file is not overwritten. This does
not change USB connection settings or the separately saved language. The physical
camera's original control is still restored on normal Stop/Close; the saved value
is reapplied on the next client preview. Server previews retain their off default.

Camera Test is modal relative to the main window on **both server and client**.
Closing the viewer re-enables its parent. If the separate preview process exits
unexpectedly, the main window detects that its window disappeared and re-enables
itself; this does not guarantee restoration of hardware controls after a crash.

The Rust window uses a small statically linked C++ DirectShow bridge built by
`Build-CameraPreview.ps1` through Cargo's build script. It needs the MSVC C++ tools
and Windows SDK already required by the Windows build. No additional runtime DLL
or video utility needs to be installed. For enumerated USB cameras the viewer loads
the standard Windows WDM KSProxy with that exact device's property bag. Other
capture sources use their normal device moniker. This avoids a confirmed Chicony
B729 failure in the DShow/MF Bridge: it stayed intermediate without delivering any
samples in the interactive session, while KSProxy delivered frames. No Windows
camera registry settings, FrameServer service settings or USB driver bindings are
changed. Capture remains exclusive to the local application.

The preview worker keeps one COM MTA alive until that worker shuts down, releasing
its capture session first. This avoids the handle accumulation reproduced when
the last MTA was repeatedly torn down and recreated between camera operations.
See the [memory and resource audit](memory-audit.md) for lifecycle measurements.

Logs identify the actual path as `capture_backend` (`windows-ksproxy` or
`device-moniker`), and `camera.first_frame` records time to the first input sample.
Relevant Microsoft documentation:
[capture formats](https://learn.microsoft.com/en-us/windows/win32/directshow/configure-the-video-output-format)
and [sample callbacks](https://learn.microsoft.com/en-us/windows/win32/directshow/using-the-sample-grabber).
Microsoft also documents the optional per-device
[DShow/MF Bridge pipeline](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/dshow-bridge-implementation-guidance-for-usb-video-class-devices).
