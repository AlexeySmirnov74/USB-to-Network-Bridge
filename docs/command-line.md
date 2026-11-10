# Command line and interface languages

For camera testing, `cameras`, `camera-modes`, `preview` and `camera-test` are
available for both roles. See [Camera preview and FPS](camera-preview.md).

`camera-test CAMERA --seconds 15 --low-light on|off` explicitly selects low-light
compensation for the test (`off` by default). It may reduce the actual FPS when
enabled. The original hardware setting is restored after the test finishes.

`usbbridge.exe` administers both roles without opening the GUI. Run `usbbridge --help`
for the complete command list. The existing `usbbridge-client.exe` transport and
diagnostic commands remain available. `usbbridge-server.exe` remains the USB transport.

Use `--config FILE` **before** the role. The server defaults to `host.local.json`
beside the executable. The client uses `client.local.json` beside the executable
when present, otherwise its existing application-data configuration.

```powershell
.\usbbridge.exe server status
.\usbbridge.exe server devices
.\usbbridge.exe server publish 2-3 on
.\usbbridge.exe server local 2-3 on
.\usbbridge.exe server publish 2-3 off
.\usbbridge.exe server forget 'USB\VID_1234&PID_5678\SERIAL'
.\usbbridge.exe server settings --auto-remove-unplugged on --unplugged-days 1
.\usbbridge.exe server settings --tcp-port 3240 --discovery-port 3241
.\usbbridge.exe server settings --backend Native
.\usbbridge.exe server settings --name "Office" --allow 192.0.2.11 --allow 192.0.2.12
.\usbbridge.exe server stop
.\usbbridge.exe server start
.\usbbridge.exe server autostart on

.\usbbridge.exe client discover
.\usbbridge.exe client add 192.0.2.10:3240 --name "Office"
.\usbbridge.exe client servers
.\usbbridge.exe client select Office
.\usbbridge.exe client pause Office
.\usbbridge.exe client resume Office
.\usbbridge.exe client devices
.\usbbridge.exe client connect 192.0.2.10:3240 2-4
.\usbbridge.exe client disconnect Office 'USB\VID_1234&PID_5678\SERIAL'
.\usbbridge.exe client enable Office 'USB\VID_1234&PID_5678\SERIAL'
.\usbbridge.exe client address Office 192.0.2.10:4000
.\usbbridge.exe client settings --discovery-port 4001
.\usbbridge.exe client proxy Office socks5 proxy.example:1080
.\usbbridge.exe client proxy Office https proxy.example:443 --auth-stdin
.\usbbridge.exe client proxy Office direct
.\usbbridge.exe client autostart on
```

`server forget ID` only removes an unplugged device from the server's descriptor
cache, publication and catalog. Physical presence is checked again before deletion;
connected devices are refused. Windows drivers/devnodes are retained. A future
physical connection adds the device again, unshared. The client keeps its own
remembered connection preferences.

Automatic cleanup defaults to enabled after **1 day** continuously unplugged.
`--auto-remove-unplugged off` disables it; `--unplugged-days N` accepts 1..36500.
History survives restarts and resets when the same USB identity returns, including
at another physical port. Enumeration failures never count as removal evidence.
Changing only these settings does not restart the transport or active USB sessions.

`--auth-stdin` reads the username and password on two UTF-8 lines from standard
input. A terminal can supply them interactively; automation should supply them
through the process's standard input. They are stored using the same Windows DPAPI
credential protection as the GUI. Passwords are never accepted as command arguments
or written to development logs. Without `--auth-stdin`, proxy authentication is off.

Server selectors accept a saved UUID, full endpoint or unique display name.
For device selectors, use the full USB identity for stable automation across physical
port changes; a current bus ID such as `2-3` is also accepted. Remembered-device
commands use the bus ID last saved by the recovery controller.

Settings commands preserve fields not specified. Repeated `--allow` options replace
the allowlist. Ports must be 1–65535; server USB and discovery/list ports must differ.
Changing the backend, name, allowlist or ports uses the same elevated, transactional
helper as the GUI and restarts the transport. Sharing a device, changing its idle
preference or changing the interface language does not restart the transport.
The server's **Enabled** checkbox controls **local use while no client is connected**;
publication is controlled separately by **Share / Stop sharing**.

Most commands return JSON. Identity, configuration keys and protocol state values
remain stable regardless of display language. Errors return a nonzero exit code.
`discover`, `list`, `connect`, `detach`, `ports` and `status` on the client delegate
to its existing transport command, preserving its output and error behavior.
`devices`/`refresh` also show unpublished devices through the server's catalog.
`logs`, `about`, `gui`, `hide`, `close-ui` and `autostart status|on|off` are available
for both roles. `hide` minimizes the window to the tray; `close-ui` also closes the
tray interface. Both leave background USB transport and recovery running.
Window commands require the same interactive Windows session as the interface.
`connect BUSID` uses the selected saved server; `connect SERVER BUSID` also accepts
its UUID or unique saved name.

Selecting a server node in the client tree makes **Disconnect** apply to all of
that server's USB imports on this client. It saves a per-server pause, detaches
the devices and stops their automatic reconnection. **Connect** resumes the
previously enabled remembered devices; devices individually disabled before the
pause remain disabled. Other server profiles retain their connections. These
actions also appear in the server-node context menu and correspond to `pause`
and `resume` in the console. The remote server service and its other clients keep
running. The pause survives application and Windows restarts.

## Languages

English is the default, independently of the Windows display language. Both GUIs
and tray menus offer **Language → English / Русский / Deutsch / Français / 简体中文**.
The preference is saved independently for each role/configuration in
`*.preferences.json`; it never changes USB settings or device names.

```powershell
.\usbbridge.exe server language en
.\usbbridge.exe client language de
.\usbbridge.exe client language fr
.\usbbridge.exe client language zh
.\usbbridge.exe client language ru
```

An open interface updates automatically. Applying a language recreates only its
windows and notification icon; the background recovery controller, driver and
USB transport keep running. If a settings dialog, an operation or unsaved server
settings are active, a change requested from the console waits until the interface
is ready. A GUI language change asks to save unsaved settings first.
`--language CODE` overrides only the console invocation's display language; use
the `language CODE` command to save a preference for the GUI.

Translations are embedded from `src/locales.json`. Display templates and parameter
values are processed separately, so USB names, addresses and error details are not
translated accidentally. Driver/system error details and developer diagnostics retain
their original language. Automated checks verify catalog completeness and matching
format parameters in all five languages.

## Video to a USB/UVC camera

Both roles accept `video settings`, `video status`, `video on`, `video off`, and `video configure FILE`. Server commands manage reception into the local USB/UVC camera; client commands manage a source sending to that receiver. Configuration uses the same elevated helper and protected-key handling as the GUI. See [video settings and examples](virtual-camera.md).
