# Transports

A job is just bytes. How those bytes reach the print head decides whether a
driver is involved. The app's preference order, and why:

| Route | Target | Driver involved | Setup |
| --- | --- | --- | --- |
| Direct USB | `usb:\\?\usb#vid_09c6&pid_0426#…` | None: inbox `usbprint.sys` only | None |
| Bluetooth LE | the app's BLE session | None | None (no pairing, no PIN) |
| Bluetooth Classic, Linux | `rfcomm:DC:1D:30:87:F2:CE`, or the app's BLE session | None | None (no pairing, no root) |
| Print queue | `raw:BY-482BT-RAW` | A queue driver, e.g. *Generic / Text Only* | Admin, once |
| Bluetooth Classic | `serial:COM8` | Inbox SPP serial driver | Windows pairing, with PIN |
| File | `file:out\job.tspl` | n/a | None |

## Direct USB (preferred)

Plugging the printer in makes Windows bind its USB printer-class interface to
the inbox `usbprint.sys` and enumerate it as
`USBPRINT\PEDOOLO_BY-482BT\…\USB001`. That driver ships with Windows and is
not vendor software. It publishes a device interface
(`GUID_DEVINTERFACE_USBPRINT`) that any program can open and write:

```text
SetupDiGetClassDevsW(GUID_DEVINTERFACE_USBPRINT, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
  SetupDiGetDeviceInterfaceDetailW  -> \\?\usb#vid_09c6&pid_0426#482b11242604#{28d78fad-…}
  SetupDiOpenDeviceInterfaceRegKey  -> "Port Name" = USB001  (links it to any queue on that port)
CreateFileW(path) + DeviceIoControl(IOCTL_USBPRINT_GET_1284_ID) -> MFG:PEDOOLO;MDL:BY-482BT;CMD:TSPL;
CreateFileW(path) + WriteFile(job)                              -> prints
```

No queue, no spooler, no printer driver, no administrator rights. The
IEEE 1284 id names the printer and confirms it speaks TSPL, which is how the
app recognises a label printer it has never seen.

Implementation: [`os/windows.rs`](../crates/tspl-core/src/os/windows.rs).
On Linux the same role is played by `/dev/usb/lpN` (the `usblp` driver), with
the 1284 id read from `/sys/class/usbmisc/lpN/device/ieee1284_id`.

The earlier hardware test printed through the `BY-482BT-RAW` queue, which uses
Microsoft's *Generic / Text Only* driver. That works, but it is still a driver
and needs an elevated `Add-Printer`. The direct route needs neither, and the
app now prefers it. A queue on the same port is hidden as a duplicate.

## Bluetooth, and the PIN prompt

The BY-482BT has two radios in one: Bluetooth **Classic** (SPP, a serial
cable over the air) and Bluetooth **LE** (GATT).

- **Windows Settings → Add device** pairs over *Classic*. Classic pairing of a
  device without a screen falls back to a legacy PIN, hence the prompt. These
  printers almost always use `0000` or `1234`. Once paired, Windows creates an
  outgoing COM port, which the app shows as a Bluetooth printer
  (`serial:COMx`), named from the paired device.
- **Label Expert on iOS** uses *LE*. iOS does not allow Classic SPP for
  ordinary apps, and LE needs no pairing, so there is no PIN.

The app uses LE, like the phone app: **Scan** then **Connect** in the
Bluetooth card, with no pairing and no PIN. Do not pair the printer in
Windows settings for this. If it is already paired and LE fails, remove it from
Windows Bluetooth settings and scan again.

On Linux, LE also needs bluetoothd to publish the printer's GATT services; see
[LE on Linux, and the Classic fallback](#le-on-linux-and-the-classic-fallback)
below.

Implementation: [`crates/label-printer/src/ble.rs`](../crates/label-printer/src/ble.rs).

1. Scan for advertisers; printers sort first.
2. Connect and pick the print characteristic: `0xFF02`, the ISSC transparent
   UART (`49535343-8841-…`), `0xAE01`, the Gprinter channel, then any writable
   characteristic outside the standard GATT services. The old code took the
   first writable characteristic, often the GAP *Device Name*.
3. Write 180-byte chunks, with acknowledged writes when supported (flow
   control), otherwise 20 ms apart.

### LE on Linux, and the Classic fallback

On Windows and iOS the LE path is all there is to it. On Linux, bluetoothd
5.88 (Fedora 45) breaks it for this printer: the LE link comes up, the printer
answers service discovery, and bluetoothd still exports no services, so every
central sees zero characteristics. The full evidence, addresses and GATT
handles are in [protocol.md §4](protocol.md#on-linux-bluez-hw).

So on Linux the app prints over the printer's Bluetooth Classic serial
channel instead. The printer is dual-mode and shows up twice in a scan,
`BY-482BT_F2CE-LE` and its Classic twin `BY-482BT_F2CE` at a separate public
address. Connecting to the LE entry looks up the twin and opens an RFCOMM
socket to it on SPP channel 1. Nothing needs setting up first:

* no pairing and no PIN — the printer accepts the connection unpaired;
* no `rfcomm bind`, no `/dev/rfcomm*` node and no root — the app opens the
  socket itself;
* the remembered printer is stored by its Classic address, so reconnecting
  after a restart needs no scan.

If the Classic side does not answer, the app falls back to LE. The activity
log shows which one it used ("print channel Bluetooth Classic serial
(RFCOMM) at …" versus a GATT UUID).

The CLI takes the same route:

```bash
tspl-rs print --target rfcomm:DC:1D:30:87:F2:CE --pattern testcard
tspl-rs print --target rfcomm:DC:1D:30:87:F2:CE,2   # another channel
```

`serial:/dev/rfcomm0` still works for anyone who binds a node by hand.

Implementation: [`crates/tspl-core/src/rfcomm.rs`](../crates/tspl-core/src/rfcomm.rs).

## Print queues (optional)

A RAW spool job bypasses the print processor and the driver's rendering code,
so the port receives the bytes unmodified. A queue is only needed if you want
the spooler (job history, sharing).

```text
OpenPrinterW -> StartDocPrinterW(DOC_INFO_1 { datatype = "RAW" })
  -> StartPagePrinter -> WritePrinter -> EndPagePrinter -> EndDocPrinter -> ClosePrinter
```

Discovery reads each queue's port and driver (`PRINTER_INFO_2`). A queue on a
`USBnnn` port with no device behind it shows as offline.

On macOS and Linux the equivalent is `lp -d QUEUE -o raw`, with
`lpstat -e` / `lpstat -v` for discovery.

## Serial ports

`serial:COM5` writes straight to the port (`\\.\COM5` on Windows).
Discovery reads each port's friendly name and PnP id from SetupAPI:

| PnP instance | Shown as |
| --- | --- |
| `BTHENUM\{00001101…}…&E8EECC06FE6E_…` | Bluetooth printer, named from the paired device |
| `BTHENUM\{00001101…}…&000000000000_…` | Dropped: incoming port, never reaches a printer |
| `USB\VID_…` or `ACPI\PNP0501` | Serial port, hidden unless it looks like a printer |

## Discovery summary

| Source | Windows | Unix |
| --- | --- | --- |
| USB printers | SetupAPI, `GUID_DEVINTERFACE_USBPRINT` | `/dev/usb/lp*` + sysfs |
| Queues | `EnumPrintersW` level 2 | `lpstat -e`, `lpstat -v` |
| Serial / SPP | SetupAPI `Ports` class + `BTHPORT` registry | `/dev` names |
| Bluetooth LE | UI, `btleplug` | UI, `btleplug` |

Each source fails independently into a warning.
[`assemble`](../crates/tspl-core/src/discover.rs) links, names, filters and
ranks the results; [`best_printer`](../crates/tspl-core/src/discover.rs)
chooses what to connect to.

## Failure modes

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Printer missing from the list | Off, unplugged, or USB cable is charge-only | Switch on; try another cable |
| `open USB printer …: Access is denied` | Another program holds the device | Close the vendor utility |
| Queue shows *offline* | No device on its USB port | Plug in and switch on; the USB entry appears |
| Literal `SIZE 101.60 mm` on paper | Queue driver is not pass-through | Use direct USB, or *Generic / Text Only* |
| BLE: `published no GATT services` | bluetoothd did not export the printer's table (BlueZ 5.88 on Linux) | Use the Classic SPP node (`serial:/dev/rfcomm0`); see the LE section above |
| BLE: `no printer data channel` | Not a printer, or only standard GATT services | Pair in Windows (PIN `0000`/`1234`), use the COM port |
| BLE connects, prints nothing | Printer asleep, or job cancelled part-way | Press FEED, then print again |
| Every other label blank | Gap setting | Use **Calibrate** |

## Security notes

- Nothing needs administrator rights, at install or print time.
- A RAW job can be sent to any queue, including an office printer, which will
  print garbage. The app never picks a non-label printer by itself.
- No network exposure: transports are local (USB, Bluetooth, serial, spooler).
