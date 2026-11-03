# USB identifiers and application compatibility

The Native server forwards USB device, configuration and string descriptor data.
The production USBIP-WIN2 client is attached with an **empty serial override**,
which preserves the device's physical `iSerialNumber` instead of supplying a
replacement. The driver's optional 15-character override limit does not truncate
the original serial descriptor when no override is requested.

Two physical cameras were compared between the Windows server and client on
2026-11-03. Their VID, PID, revision, hardware IDs, compatible IDs and serial-bearing
parent device instance IDs matched. The external camera's ContainerID also matched.
The built-in camera's server-side reserved/internal ContainerID differed from the
client's generated ContainerID. Composite-function instance paths, parent hubs and
physical location paths differed because the client uses its own virtual USB bus.

| Identifier used by an application | Result / limit |
| --- | --- |
| USB VID, PID, revision and hardware IDs | Preserved; verified on both cameras |
| Physical USB serial number | Preserved; verified on both cameras |
| Parent instance ID containing a unique USB serial | Matched on both tested cameras |
| ContainerID | Can match for external serialized USB; can differ for built-in devices |
| Composite interface path, parent hub, port or location path | Depends on the client's Windows topology |
| Device without a unique physical serial number | Windows may generate an identity from its location |
| Host machine identity or application activation state | Separate from USB forwarding |

Applications selecting a camera by a saved local interface path may need to select
it again on the client. Preserving USB identifiers supports software that identifies
the hardware using those identifiers; it does not establish compatibility with every
application's device binding. No Windows device identity registry entries are
rewritten and no artificial serial number is introduced.

Windows' generation rules are described in
[USB ContainerIDs](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/usb-containerids-in-windows)
and [ContainerID generation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/how-container-ids-are-generated).
