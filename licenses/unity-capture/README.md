# Unity Capture DirectShow filter

The unmodified x64 filter is included in the offline portable ZIP and MSI under
`app/components/unity-capture/UnityCaptureFilter64.dll`. It is a userspace COM/DirectShow component, not a kernel driver.
It has no dynamic Visual C++ runtime dependency. Its exact SHA-256 and upstream
commit are recorded in `unity-capture.json`.

Upstream: https://github.com/schellingb/UnityCapture

Source: https://github.com/schellingb/UnityCapture/tree/3ed54c325e0ad71afcf4f246c07e5e17b3d7f2d2/Source

The filter is MIT licensed; `LICENSE.txt` preserves the upstream notice. The
separate Unity producer plugin/assets use the upstream zlib license and are not
redistributed here. Producer applications such as OBS and DeepLive are external.
Do not change or redistribute those components without their applicable notices.

Build input retrieval is explicit: `Download-VideoComponents.ps1`. Runtime setup
and repair never download files. New registrations use a protected permanent
ProgramData copy, so moving the portable application does not break COM paths.
Already valid external registrations are reused without replacement.
