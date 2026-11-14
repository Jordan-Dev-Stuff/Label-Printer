# Roadmap

Each step has an acceptance check that can be run without reading code.

## Step 1 — engine and CLI (done)

- [x] `tspl-core`: frames, Floyd–Steinberg, TSPL encoder, targets, discovery
- [x] `tspl-cli` with `list`, `print`, `send`
- [x] Zero dependencies in the core; builds with `--offline`

**Acceptance:** `cargo test` passes; `tspl-rs print --target file:...` writes a
124,371-byte 4x6 job whose first line is `SIZE 101.60 mm,152.40 mm`.

## Step 2 — driverless discovery (done)

- [x] Direct USB through `usbprint.sys` (`usb:` target), with IEEE 1284 id
- [x] Queues linked to their USB device; unplugged printers show offline
- [x] Bluetooth serial ports named from the paired device; incoming ports dropped
- [x] Virtual queues and non-printer ports hidden
- [x] `best_printer`: last used, then direct USB, queue, Bluetooth serial
- [x] Print through `usb:` with no driver involved (confirmed on a BY-482BT)

**Acceptance:** with the `BY-482BT-RAW` queue deleted, `tspl-rs list` shows the
printer as `USB` and `tspl-rs print --target usb:<path>` prints the test card.

## Step 3 — desktop app (done)

- [x] Dark UI: header, cards, toggles, status bar, Phosphor icons
- [x] Background workers for discovery, rendering/printing and Bluetooth
- [x] Automatic connection, settings remembered between runs
- [x] Label presets, rotation, halftone choice, invert, Feed, Calibrate
- [x] Drop zone when nothing is open; screenshots in `docs/screenshots/`

## Step 4 — PDF (done)

- [x] pdfium embedded in the executable; no runtime download
- [x] Pinned pdfium 7543 matching the `pdfium-render` bindings
- [x] Two-pass render: content found, then rendered at exactly 203 dpi
- [x] Every page of a multi-page PDF in one job
- [x] `--render` headless check and a Letter-page fixture

## Step 5 — Bluetooth LE printing (done)

- [x] Characteristic selection by known printer UUIDs
- [x] Chunked writes with flow control, progress and cancel
- [x] Remember the printer, reconnect automatically
- [ ] Confirm reconnect on start (15 s scan, quiet retries) on hardware
- [x] Prints on a BY-482BT over LE with no pairing (confirmed)
- [x] Progress, time left and cancel; optional faster streamed mode

**Acceptance:** a label prints over Bluetooth with the USB cable unplugged and
the printer never paired in Windows.

## Step 6 — release

- [x] Inno Setup script (`installer/thermal-rs.iss`), single-file install
- [x] CI (fmt, clippy, tests on Windows and Linux) and a tag-triggered release
  workflow: installer, portable zip, checksums
- [ ] Code signing

## Open questions

1. **Compressed `BITMAP`.** Bluetooth sends of ~120 KB are slow; the BT
   models support a compressed raster (vendor table: `compress="true"`). The
   format needs to be recovered from the vendor driver package.
2. **Fast-mode pacing.** Streamed writes with an acknowledged sync every 16
   chunks; confirm on hardware and tune.

## Non-goals

- A Windows print driver of any kind.
- Network printing (IPP).
- Printer-side fonts and barcodes; host rendering is the compatibility strategy.
