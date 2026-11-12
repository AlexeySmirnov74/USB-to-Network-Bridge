# Installing USB to Network Bridge

Supported runtime: **Windows 11 x64 with the standard Windows media components**. Windows 11 N requires Microsoft’s Media Feature Pack to be installed beforehand; the offline Bridge package cannot supply missing Windows optional features. See [Microsoft’s Windows N guidance](https://support.microsoft.com/en-us/windows/experience/platform-variants/media-feature-pack-for-windows-n).

Windows 11 x64 is the initial target. The MSI and project EXEs are intentionally unsigned. The complete MSI and portable ZIP include both original signed USB driver installers. Setup and repair use local files with pinned SHA-256 and cache-only Windows Authenticode verification; they never download drivers. No test signing or automatic Windows restart is used.

## MSI

1. Open `USB-to-Network-Bridge-<version>-windows-x64.msi`. Choose Server on the computer with physical USB devices and Client on the computer that will use them. Both roles may be installed.
2. Open **USB to Network Bridge → Server** or **Client** in the Start menu. Launching checks required drivers automatically; the first setup or a missing driver requests administrator rights to configure the selected role and register startup tasks. Installed drivers are reused.
3. Restart Windows manually if driver setup requests it. Start menu shortcuts remain available.
4. On the server, open Settings and set the allowed client LAN/VPN IP addresses. A fresh server starts disabled with only loopback permitted and no devices shared. Enable the server, then share selected devices.
5. On the client, discover the server or enter its IP/hostname and ports, then connect the required devices.

The two desktop launchers reside in `%ProgramFiles%\USB to Network Bridge`; the internal EXEs, helpers, documentation and licenses are in its `app` subdirectory. Role configuration is stored under `%ProgramData%\USB to Network Bridge\Server` or `Client`. Server configuration changes use the elevated helper; GUI language preferences and normal-user diagnostic fallback logs are under `%LocalAppData%\USB to Network Bridge`. Client configuration is writable by local users; use a trusted Windows account.

The setup refuses to take over a role whose existing startup task belongs to another executable path. An installed copy and a portable copy should not control the same role concurrently.

## Repair, update and remove

Use Installed apps or `msiexec`:

```powershell
msiexec /i USB-to-Network-Bridge-<version>-windows-x64.msi
msiexec /fa "{PRODUCT-CODE}"
msiexec /x "{PRODUCT-CODE}"
```

The product code is listed in the MSI's build metadata and Windows Installer registration. Installer maintenance stops only tasks/processes belonging to its own executable path. Repair/update resumes the installation's previously running roles. Uninstall removes its files, owned startup tasks and owned firewall rules; private configuration/logs and shared upstream USB driver installations are retained. A live device session belonging to the updated/uninstalled installation is interrupted.

An upgrade from the previous flat layout stops its owned tasks before replacing files, migrates their executable/configuration references to `app`, then resumes the previously active roles. Existing private configuration is migrated rather than replaced by a packaged example. The root remains the registered installation directory and shortcut target.

## Portable ZIP

Extract the complete ZIP, for example to `C:\USBBridge`. The package root contains only:

```text
USB-to-Network-Bridge-Server.exe
USB-to-Network-Bridge-Client.exe
app/
```

Double-click the required launcher to open its GUI without a console window. Saved role settings are reused, and an existing interface is activated instead of starting another instance. Keep the complete `app` directory beside the launchers. It contains five internal executables, runtime PowerShell helpers, examples, documentation, licenses and `SHA256.json`. Hash manifest paths are relative to the package root, including the two desktop EXEs. The advanced command-line tool is `app\usbbridge.exe`; an optional launcher `--config FILE` selects an existing custom configuration.

First launch performs role setup automatically. Missing official drivers are installed from the verified local files under `app\tools\driver-installers`; client setup also verifies and registers the bundled x64 Unity Capture sources. Approve UAC when required. Reopen the launcher after any manually requested Windows restart. No automatic reboot or changes to Secure Boot, memory integrity or driver signature mode are performed.

Fresh setup protects the runtime code in `app` against replacement by ordinary users before registering new administrator/SYSTEM startup tasks. Broadly writable parent locations are rejected during fresh setup; use a normal local installation folder rather than a shared writable directory. A fresh portable client stores its copyable configuration under `app\data\Client`; a fresh server uses the protected ProgramData role directory. A same-PC migration of an existing flat deployment retains its settings and existing trust, and rebases the owned tasks instead of resetting the role. Do not replace an existing deployment's configuration with example files.

### Moving a folder on the same Windows installation

Close the application windows, move both launchers together with the entire `app` directory, then open the launcher from the new location. The first launch requests UAC to rebind only this installation's startup tasks, firewall program paths and shortcuts. Existing settings, device choices, generated-camera serials and key bytes are retained; the old folder need not exist. Disabled startup tasks remain disabled.

Filesystem settings use paths relative to the JSON that contains them, independently of the working directory. `@app` anchors the current internal executable directory; `@programdata` anchors Windows common application data. For example, a host configuration can contain `"runtime": "@app"` and `"profile": "server.local.psd1"`, and a video configuration can reference `"key_file": "video-credentials/camera.key"`. Camera monikers and USB device identities are not filesystem paths and remain unchanged. Legacy absolute settings are accepted and upgraded once. Keep files referenced by relative paths with the folder; imported external credential files are copied locally without changing their bytes.

Windows requires absolute executable paths in Task Scheduler, firewall rules and shortcuts. These OS registrations are tracked separately from copyable settings and updated on launch. A custom main configuration outside `app` and ProgramData remains at its registered external location; copy it into the application data folder if it must travel with the portable folder. MSI installations should be moved by reinstalling to the desired location so Windows Installer also tracks the destination.

### Moving a portable copy to another PC

Copy both launchers and the complete `app` folder. Machine identity is checked for each role; a fresh copy is prepared for the destination Windows installation, and driver readiness is checked there. The server receives fresh machine-specific setup rather than publishing the previous PC's USB inventory automatically.

A previously configured client on another PC asks whether to **keep saved server connections** or use **new settings**; **Cancel** makes no changes. Keep retains saved servers, device choices and preferences, and clears machine-bound credentials, keys, stale attachments and local-camera bindings. Re-enter proxy/video secrets and configure the destination's local camera again. New starts with fresh client settings. A missing driver may require UAC and a manual reboot even when the source PC already had it installed.

Setup does not take over startup tasks owned by another installation path. Keep one active deployment per role. See the [quick start](../README.md#quick-start) for normal GUI operation.

## Developer qualification

See [build and release guide](production-readiness.md) for installer testing, resource/camera observations and asset verification. Qualification commands are developer tools, not part of normal first-run configuration.

## Exit and reopen

Both role windows and tray menus provide **Exit application**. It releases that role’s USB connections and stops its controllers, supervisor and associated video processes. Autostart preferences remain unchanged; scheduled checks honor the exit during the current Windows boot. Reopening the role launcher resumes its enabled tasks. A Windows restart also permits configured startup. The window close button continues to hide the interface. Stopping SYSTEM processes or resuming their tasks may request UAC.

Full exit logs phase durations in `application.quit` and the complete helper duration in `application.lifecycle.complete`. Shutdown waits for owned USB cleanup; it does not add a fixed grace period for inactive workers. Normal UAC approval time is outside the privileged helper measurement.

## Offline installation on another computer

Copy both desktop EXEs together with the entire `app` directory. The matching MSI contains the same offline payload. No compiler, GitHub access or internet connection is needed for USB Bridge setup or repair on a supported Windows 11 x64 installation.

```text
USB-to-Network-Bridge-Server.exe
USB-to-Network-Bridge-Client.exe
app/
  tools/driver-installers/
    usbipd-win_5.3.0_x64.msi
    USBip-0.9.8.1-x64.exe
  components/unity-capture/
    UnityCaptureFilter64.dll
```

The server uses usbipd-win's signed VBoxUSB capture drivers; the client and each generated-camera receiver use USBIP-WIN2. Its unchanged installer also contains the Visual C++ runtime needed by upstream utilities. Existing working drivers are reused. UAC and a manual Windows restart can still be necessary; no automatic reboot or test-signing changes occur.

Keep the complete folder. A missing, modified, wrongly signed or untrusted installer stops setup with a local error; setup will not silently fetch a replacement. Restore the files from the same verified release package. `Download-Drivers.ps1` is an explicit developer preparation command, never called by normal first-run setup or repair. Driver signatures are checked against Windows' installed trust store/cache without fetching certificates or revocation lists.

The complete package also contains the x64 Unity Capture filter used by the two client video routes. Client setup registers **Unity Capture Server** (slot 0) and **Unity Capture Client** (slot 1) locally. Enabling the client USB camera repairs a missing source before changing its endpoints. To check or repair explicitly, close video applications and run `Ensure-VideoPrerequisites.ps1 -Role Client -RuntimeDirectory . -Elevate` from `app` in 64-bit Windows PowerShell. Existing compatible registrations are reused; ambiguous or foreign registrations are not silently replaced.

OBS and the application producing video, such as DeepLive or a Unity project, are separate products; Bridge does not install or download them. Windows/device-vendor drivers needed by particular forwarded equipment also remain their respective products. See [driver licenses and corresponding source](driver-sources.md) and [third-party notices](../THIRD-PARTY.txt).

The local verifier also contains the pinned GlobalSign intermediate CA missing from the original client EXE. After verifying the exact installer hash, it can cache only this non-root issuing certificate in the current user CA store. It does not add trusted roots or TrustedPeople entries, bypass Authenticode checks, or fetch certificate URLs. Windows must already trust the upstream root; a missing or disallowed root remains an error.
