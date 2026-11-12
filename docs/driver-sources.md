# Upstream driver licenses and source

The offline package contains two original, signed upstream installers. USB to Network Bridge does not modify their EXEs, MSIs, catalog files or kernel drivers. Its Rust application source remains under the project MIT license; the upstream components keep their own licenses.

| Component | Exact version | Upstream license | Source |
| --- | --- | --- | --- |
| usbipd-win | 5.3.0 (tag `v5.3.0`) | GPL-3.0-only | [Pinned source archive](https://github.com/dorssel/usbipd-win/archive/refs/tags/v5.3.0.zip) |
| VBoxUSB / VBoxUSBMon, included by usbipd-win | VirtualBox 7.2.2 | GPL-3.0-only | [Complete official source archive](https://download.virtualbox.org/virtualbox/7.2.2/VirtualBox-7.2.2.tar.bz2) |
| USBIP-WIN2 | 0.9.8.1 (tag `v.0.9.8.1`) | BSD-2-Clause | [Pinned source archive](https://github.com/vadimgrn/usbip-win2/archive/refs/tags/v.0.9.8.1.zip) |

The same GitHub release as the offline installer and portable ZIP provides a separate **driver-sources** archive, at no charge, containing the exact upstream source archives, source provenance and their SHA-256 hashes. Use the archive from the matching release when retaining or redistributing its driver binaries. The small USB to Network Bridge source ZIP is not a replacement for this upstream source archive. The driver source archive is not needed to install or run the application offline.

Full upstream license texts and copyright notices are shipped under `licenses/drivers/`. The usbipd-win [driver declaration](https://github.com/dorssel/usbipd-win/blob/v5.3.0/Drivers/README.md) identifies VirtualBox **7.2.2** as the source of its signed capture drivers. A usbipd-win source ZIP alone contains these binary drivers and is not the full VirtualBox corresponding source. The complete Oracle source archive is included separately in driver-sources.

Oracle publishes the expected source hash in [SHA256SUMS](https://download.virtualbox.org/virtualbox/7.2.2/SHA256SUMS):

```text
b0e63bfb855327aecf1122cda33390c33734e5ffed72f123f5edf786a8ce1393  VirtualBox-7.2.2.tar.bz2
```

The bundled USBIP-WIN2 installer also embeds Microsoft's x64 Visual C++ Redistributable for its upstream command-line tools. The original Microsoft runtime remains within that unchanged installer. The bridge's own Rust EXEs use a static CRT; they do not require installing .NET or a separate Visual C++ runtime.

The unmodified x64 **Unity Capture DirectShow filter is bundled** separately from the driver installers. `unity-capture.json` pins its upstream commit, exact DLL path and SHA-256. It is an MIT-licensed userspace component; its full notice and source URL are included under `licenses/unity-capture/`. Its upstream source is [UnityCapture at the pinned commit](https://github.com/schellingb/UnityCapture/tree/3ed54c325e0ad71afcf4f246c07e5e17b3d7f2d2/Source). The filter has no dynamic Visual C++ runtime dependency. The separate Unity producer plugin/assets (zlib), OBS and producer applications are not bundled. See [third-party video integration](third-party-video.md) and [dual camera destinations](dual-camera.md). This filter does not change the signed kernel drivers or the three-archive driver-source asset.

Maintainers can reproduce the driver-source release asset with `python Package-DriverSources.py --source-directory PATH`, where PATH contains the three original archives named in `driver-sources.json`. The packager reads only local files, verifies their pinned SHA-256 hashes, and retains the original archive bytes. It does not compile or re-sign drivers.

The local verifier also contains the pinned GlobalSign intermediate CA missing from the original client EXE. After verifying the exact installer hash, it can cache only this non-root issuing certificate in the current user CA store. It does not add trusted roots or TrustedPeople entries, bypass Authenticode checks, or fetch certificate URLs. Windows must already trust the upstream root; a missing or disallowed root remains an error.
