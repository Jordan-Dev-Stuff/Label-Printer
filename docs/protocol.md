# Printer protocol reference

Everything this project knows about talking to the PEDOOLO / BEEPRT BY-482BT
(and its relatives), from the bytes of a print job down to how the printer
appears on USB and Bluetooth. Each fact is marked with how it was
established:

| Mark | Source |
| --- | --- |
| **[hw]** | Observed on a real BY-482BT |
| **[vendor]** | Static analysis of the vendor driver package `RHEBAR_1.3.2.20.exe` (read only; the installer was never run) |
| **[tspl]** | The public TSPL/TSPL2 programming manual (TSC Auto ID) |
| **[test]** | Pinned by a test in this repository |

## 1. The printer

The BY-482BT is a 203 dpi, 4-inch direct-thermal label printer that speaks
**TSPL**, the label language TSC created and many low-cost printers copied.
It has a USB-B port and a dual-mode Bluetooth radio.

### Capability table [vendor]

The vendor installer embeds an XML device database. The relevant entries:

```xml
<Driver name="IP-592" usbname="USBPRINT\IPRT_IP-592C0FC" cmd="tspl" dpi="203"
        headtype="DT" defheadtype="DT" speeds="2,3,4,5,6" defspeed="4"
        densitys="0,15" defdensity="8" headsize="108" hmargin="3"
        minwidth="3" maxwidth="110" minlength="3" maxlength="1000"
        defgap="3,0" defpapersize="100,179" paperlist="label_rhebar"
        defpapername="100mm x 150mm" />

<Driver name="Beeprt BY-482BT" usbname="USBPRINT\Beeprt_BY-482BT8F77" parent="IP-592" compress="true" />
<Driver name="BY-482BT"        usbname="USBPRINT\BY-482BTA566"      parent="IP-592" compress="true" />
<Driver name="BY-482"          usbname="USBPRINT\Beeprt_BY-4827F0B" parent="IP-592" />
```

The BY-482BT is an `IP-592` with `compress="true"` added:

| Property | Value | Consequence |
| --- | --- | --- |
| Command set | `tspl` | ASCII commands plus a binary raster |
| Resolution | 203 dpi | 4 in = 812 dots, 6 in = 1218 dots; 1 dot = 0.125 mm |
| Head width | 108 mm | 863 dots printable |
| Media width | 3–110 mm | 101.6 mm (4 in) fits |
| Media length | 3–1000 mm | 152.4 mm (6 in) fits |
| Gap default | 3 mm, offset 0 | `GAP 3.00 mm,0.00 mm` |
| Density | 0–15, default 8 | `DENSITY 8` |
| Speed | 2–6, default 4 | `SPEED 4` |
| Head type | DT (direct thermal) | No ribbon; heat-sensitive media only |
| Compression | Bluetooth models only | See [section 6](#6-open-questions) |

### USB identity [hw]

| Item | Value |
| --- | --- |
| USB device | `USB\VID_09C6&PID_0426\482B11242604` |
| Printer-class child | `USBPRINT\PEDOOLO_BY-482BT\7&FE1AD9C&0&USB001` |
| Friendly name | `PEDOOLO  BY-482BT` (two spaces) |
| Spooler port | `USB001`, created by the *Dynamic Print Monitor* |

The `USBPRINT\` enumerator belongs to `usbprint.sys`, the printer-class driver
that ships with Windows. It binds without any vendor software, which is what
makes driverless printing possible.

### Bluetooth identity [hw]

| Item | Value |
| --- | --- |
| Radio | Dual mode: Bluetooth Classic (SPP) and Bluetooth LE (GATT) |
| LE advertised name | `BY-482BT_F2CE-LE`: model, the last two bytes of the address, `-LE` |
| Classic pairing | Asks for a legacy PIN on Windows; LE needs no pairing |

## 2. TSPL job format

A job is ASCII command lines ending in `\r\n`, with one binary blob in the
`BITMAP` command [tspl]. This is the exact job for a 4x6 label [test]:

```text
SIZE 101.60 mm,152.40 mm\r\n
GAP 3.00 mm,0.00 mm\r\n
DIRECTION 1\r\n
REFERENCE 0,0\r\n
DENSITY 8\r\n
SPEED 4\r\n
CLS\r\n
BITMAP 0,0,102,1218,0,<124236 bytes>\r\n
PRINT 1,1\r\n
```

### Commands used

| Command | Meaning |
| --- | --- |
| `SIZE w mm,h mm` | Label size. Two decimals; inches also accepted (`SIZE 4,6`) |
| `GAP g mm,o mm` | Gap between labels and its offset. `GAP 0 mm,0 mm` = continuous roll |
| `DIRECTION 1` | Print orientation relative to the feed direction |
| `REFERENCE 0,0` | Origin of the coordinate system |
| `DENSITY n` | Darkness 0–15 |
| `SPEED n` | Inches per second; 2–6 on this printer |
| `CLS` | Clear the image buffer to **white** |
| `BITMAP x,y,w,h,mode,data` | Raster image (below) |
| `PRINT n,1` | Print `n` copies of the buffer, feeding labels |
| `FORMFEED` | Feed one blank label (the app's **Feed label**) |
| `GAPDETECT` | Feed a few labels and measure label and gap with the sensor (**Calibrate**) |

**Order matters** [tspl]: `SIZE` and `GAP` configure geometry before `CLS`
clears the buffer, and `PRINT` is what actually feeds media. `BITMAP` before
`SIZE` is accepted by some firmware but prints at the previous job's geometry.

### `BITMAP x,y,width,height,mode,data`

| Field | Meaning |
| --- | --- |
| `x`, `y` | Top-left corner in dots |
| `width` | **Row width in bytes**, not dots |
| `height` | Rows |
| `mode` | `0` = overwrite |
| `data` | `width × height` bytes of packed 1-bit rows, no separators |

Each row is `ceil(dots / 8)` bytes, most significant bit = leftmost dot.

> **Polarity: a cleared bit (0) prints a black dot; a set bit (1) leaves the
> paper white.** [hw] [test]

This is the opposite of ESC/POS raster graphics and easy to get wrong: an
encoder that sets bits for black prints every label as a negative, white text
on a solid black label, over both USB and Bluetooth. An all-white row is
`0xFF` bytes.

Row padding follows from the polarity. An 812-dot row is 101.5 bytes, so it is
sent as 102 bytes (816 dots), and the 4 padding bits are **set**, so they
stay white. 816 dots is 102.0 mm against a 101.6 mm label on a 108 mm head,
so the extra dots fall on the liner anyway.

### Worked example [test]

```text
width  = 4 in x 203 = 812 dots -> 102 bytes per row
height = 6 in x 203 = 1218 rows
raster = 102 x 1218 = 124,236 bytes
header = 122 bytes (SIZE … up to and including "BITMAP 0,0,102,1218,0,")
tail   = 13 bytes ("\r\nPRINT 1,1\r\n")
total  = 124,371 bytes
```

`crates/tspl-core/tests/encode.rs` asserts this length, the header bytes, an
all-`0xFF` raster for a white label, and the bit layout for single dots.

### Compact jobs

`CLS` leaves the buffer white, so only rows containing ink need sending. With
`Config::skip_blank` (always on in the app; `--compact` in the CLI) the
encoder sends one `BITMAP` per band of inked rows, each trimmed to its inked
bytes and placed at its own `x,y`. Blank runs shorter than 8 rows stay
inside a band (a new command costs about 25 bytes). A label with a full
border saves little (every row has ink); a sparse label saves a lot.

### What the raster means

Any grey value below 128 prints black. The app converts grey to black and
white before encoding:

| Halftone | Used for | Why |
| --- | --- | --- |
| Threshold at 150 | Text, barcodes, line art | Crisp edges; dithering makes bar edges ragged and hurts scanning |
| Floyd–Steinberg | Photos, gradients | Keeps tone instead of banding |

*Auto* picks threshold when fewer than 8% of pixels are mid-grey. Author
content at final size: a 1-dot line (0.125 mm) is the thinnest mark the head
makes, and lines under ~0.25 mm are unreliable.

### Not used

| Command | Why not |
| --- | --- |
| `TEXT`, `BARCODE`, `BOX`, `QRCODE` | Rendering on the host gives identical output on every firmware |
| `PUTBMP`, `DOWNLOAD` | Need files on the printer's flash; `BITMAP` streams instead |
| `SET PEEL`, `SET CUTTER` | No accessory fitted |
| `~!T`, `~HS` (status) | Needs a read channel; jobs are fire-and-forget |
| Compressed raster | Format not yet known ([section 6](#6-open-questions)) |

## 3. USB

### Direct, no driver

`usbprint.sys` publishes a device interface for every printer-class device,
`GUID_DEVINTERFACE_USBPRINT` = `{28d78fad-5a12-11d1-ae5b-0000f803a8c2}`. Any
program can enumerate it and write to it, with no queue, no driver and no
administrator rights [hw]:

```text
SetupDiGetClassDevsW(&GUID_DEVINTERFACE_USBPRINT, NULL, NULL,
                     DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
SetupDiEnumDeviceInterfaces / SetupDiGetDeviceInterfaceDetailW
    -> \\?\usb#vid_09c6&pid_0426#482b11242604#{28d78fad-5a12-11d1-ae5b-0000f803a8c2}
CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING)
WriteFile(job)   (16 KiB at a time; usbprint splits further)
```

Extra facts read along the way:

| Query | Result |
| --- | --- |
| `SetupDiOpenDeviceInterfaceRegKey` → value `Port Name` | `USB001`: links the device to any spooler queue on that port |
| `SetupDiGetDevicePropertyW(DEVPKEY_Device_BusReportedDeviceDesc)` | USB product string |
| `DeviceIoControl(IOCTL_USBPRINT_GET_1284_ID)` | IEEE 1284 device id |

`IOCTL_USBPRINT_GET_1284_ID` is `0x220034`
(`CTL_CODE(FILE_DEVICE_UNKNOWN, 13, METHOD_BUFFERED, FILE_ANY_ACCESS)`). The
reply is a **big-endian 16-bit length that counts itself**, then ASCII
`KEY:value;` pairs such as `MFG:…;MDL:…;CMD:TSPL;`. `CMD` names the command
set, which is how an unknown printer is recognised as a label printer
([`decode_1284_reply`, `parse_device_id`](../crates/tspl-core/src/discover.rs)).
The id is cached per device path, so discovery does not keep opening the
printer.

Linux exposes the same thing as `/dev/usb/lpN` (driver `usblp`), with the
1284 id in `/sys/class/usbmisc/lpN/device/ieee1284_id`.

### Through a print queue (optional)

A queue on `USB001` with the inbox *Generic / Text Only* driver also works
[hw], provided the job is submitted with the `RAW` datatype so the spooler
passes bytes through untouched:

```text
OpenPrinterW -> StartDocPrinterW(DOC_INFO_1 { pDatatype = "RAW" })
  -> StartPagePrinter -> WritePrinter -> EndPagePrinter -> EndDocPrinter -> ClosePrinter
```

That needs an elevated `Add-Printer` and is still a driver, so the app prefers
the direct route and hides a queue whose USB device is present as a
duplicate. A queue on a `USBnnn` port with no device behind it is reported
offline.

## 4. Bluetooth

### Classic vs LE, and the PIN

The radio is dual mode [hw]:

| | Bluetooth Classic (SPP) | Bluetooth LE (GATT) |
| --- | --- | --- |
| Used by | Windows *Settings → Add device* | Phone apps (Label Expert on iOS), this app |
| Pairing | Required; a printer without a screen falls back to a **legacy PIN** | None |
| PIN | Usually `0000` or `1234`; the manual does not say | Not applicable |
| On Windows | A virtual COM port ("Standard Serial over Bluetooth link") | Direct GATT writes |
| Data | Raw TSPL bytes over the serial link | Raw TSPL bytes written to a characteristic |

iOS does not let ordinary apps use Classic SPP, which is why the phone app
never asks for a PIN: it only ever uses LE. Windows has no way to connect to
an LE device by address without scanning first, so the app scans (up to 15 s,
stopping as soon as the remembered printer appears) and then connects.

### LE print channel

The printer exposes the job as a writable GATT characteristic; there is no
LE-specific framing. The same TSPL bytes as USB are written in chunks.
Characteristics tried, in order ([`choose_channel`](../crates/label-printer/src/ble.rs)):

| UUID | Commonly found on |
| --- | --- |
| `0000ff02-0000-1000-8000-00805f9b34fb` | 0xFF00 service: Phomemo and many TSPL / ESC-POS printers |
| `49535343-8841-43f4-a8d4-ecbe34729bb3` | Microchip/ISSC "transparent UART", common in BEEPRT units |
| `0000ae01-0000-1000-8000-00805f9b34fb` | 0xAE30 service of generic label printers |
| `bef8d6c9-9c21-4c9e-b632-bd58c1009f9f` | Gprinter / Xprinter data channel |
| `0000ff01-0000-1000-8000-00805f9b34fb` | Older 0xFF00 variants |
| any other writable characteristic | Outside the standard services `0x1800`, `0x1801`, `0x180A`, `0x180F` |

Picking "the first writable characteristic" is a trap: Generic Access
(`0x1800`) often has a **writable Device Name** (`0x2A00`). Bytes written there can
rename the printer and never print. The Activity log records which
channel was chosen for a given printer.

### Pacing [hw]

| Mode | Writes | Notes |
| --- | --- | --- |
| Reliable (default) | 180-byte chunks, each acknowledged (*Write With Response*) | The printer paces the transfer; slow but never overruns |
| Faster sending | 180-byte unacknowledged chunks 4 ms apart, every 16th acknowledged | The periodic acknowledgement is the flow control |
| Channel without acknowledged writes | Unacknowledged, 20 ms apart | |

A 4x6 label is ~120 KB, so Bluetooth printing takes noticeably longer than
USB; the app shows progress, time left and Cancel. Cancelling mid-job can
leave a partial label in the printer; **Feed label** clears it.

## 5. How the app recognises a label printer

[`assemble`](../crates/tspl-core/src/discover.rs) combines queues, USB
devices and serial ports, then ranks them
([`best_printer`](../crates/tspl-core/src/discover.rs)):

1. The printer used last time, if online.
2. Direct USB label printer.
3. Queue whose USB label printer is present, then any label-printer queue.
4. Paired Bluetooth serial port whose device name looks like a printer.

"Label printer" means the 1284 `CMD` names a label language (TSPL, ZPL, EPL,
DPL, CPCL), or a name matches known brands and models (`by-482`, `beeprt`,
`pedoolo`, `zebra`, `phomemo`, …). Nothing else is ever chosen automatically,
so TSPL never goes to an office printer by accident. Bluetooth LE printers
are remembered by id and name and reconnected the same way.

## 6. Open questions

1. **Compressed raster.** The vendor table enables `compress="true"` only for
   the Bluetooth models, which suggests compression exists to save radio
   bandwidth. The format (a `BITMAP` mode, or a separate command) still needs
   to be recovered from the vendor package. It would make Bluetooth printing
   several times faster.
2. **Status back-channel.** `~!T` / `~HS` over USB bulk-in or a notify
   characteristic would confirm that a job finished.
3. **LE pacing limits.** How fast the printer can take unacknowledged writes
   before its buffer overruns.

## References

- TSPL/TSPL2 Programming Manual, TSC Auto ID: authoritative command syntax.
- Vendor driver package `RHEBAR_1.3.2.20.exe`: device capability table.
- Microsoft: *USB Printing Support* (`usbprint.sys`), `GUID_DEVINTERFACE_USBPRINT`,
  `IOCTL_USBPRINT_GET_1284_ID`.
- IEEE 1284 device ID format (`KEY:value;` pairs).
