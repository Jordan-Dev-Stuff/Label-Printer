# Command line

`tspl-rs` is the test harness and the fallback when the app is unavailable. It
uses the same engine as the app. It is installed next to `thermal-ui.exe`.

## Commands

```text
tspl-rs list [--all]
tspl-rs print --target <T> [options]
tspl-rs send  --target <T> --job <FILE>
```

### `list`

```powershell
tspl-rs list
```

```text
* USB         PEDOOLO BY-482BT
    usb:\\?\usb#vid_09c6&pid_0426#482b11242604#{28d78fad-5a12-11d1-ae5b-0000f803a8c2}
    Direct USB · no driver · TSPL · USB001
(3 more hidden: virtual queues and non-printer ports; --all shows them)
* = chosen automatically by the UI
```

Each printer shows its connection type, name, target and details. Offline
printers (a queue whose USB printer is unplugged) are marked `[offline]`.
`--all` also lists PDF/XPS queues, serial ports that are not printers and the
duplicate queue of a USB printer. Warnings about sources that could not be
queried go to stderr.

### `print`

Renders a pattern (or a PGM file), halftones it, encodes and sends it.

```powershell
# 4x6 test card to a file, no printer needed
tspl-rs print --target file:out\card.tspl --pattern testcard

# Print it over USB (copy the target from `list`)
tspl-rs print --target "usb:\\?\usb#vid_09c6&pid_0426#…" --pattern testcard

# Darker, slower, three copies, only the inked bands sent
tspl-rs print --target "usb:…" --pattern testcard --density 12 --speed 2 --copies 3 --compact

# Different media
tspl-rs print --target serial:COM5 --width 76.2 --height 127 --gap 2
```

| Flag | Default | Range | Notes |
| --- | --- | --- | --- |
| `--pattern` | `testcard` | `testcard`, `checker` | Ignored when `--pgm` is given |
| `--pgm` | — | path | Binary PGM (P5), 8-bit |
| `--width` | `101.6` | 0–108 mm | Label width |
| `--height` | `152.4` | ≥ 0 mm | Label height |
| `--dpi` | `203` | 50–1200 | Affects pixel maths only |
| `--density` | `8` | 1–15 | Print darkness |
| `--speed` | `4` | 1–12 | Print speed (the BY-482BT accepts 2–6) |
| `--gap` | `3` | ≥ 0 mm | 0 = continuous media |
| `--copies` | `1` | 1–99 | `PRINT n,1` |
| `--compact` | off | flag | Send only bands of rows with ink (see [protocol.md](protocol.md#compact-jobs)) |

### `send`

Delivers an existing job file unchanged, e.g. a captured job:

```powershell
tspl-rs send --target "usb:…" --job captured.prn
```

## Targets

| Form | Destination |
| --- | --- |
| `usb:PATH` | USB printer, written directly: a `usbprint` interface path on Windows, `/dev/usb/lpN` on Linux |
| `raw:QUEUE` | Print queue as a RAW job (Windows) or `lp -o raw` (Unix) |
| `serial:PORT` | Serial port or device node, e.g. `COM5`, `/dev/rfcomm0` |
| `file:PATH` | A file, creating parent folders |

Only the first colon separates kind and value, so `file:C:\temp\job.tspl` and
`usb:\\?\usb#…` work as written. Quote USB paths in PowerShell because of the
`#` and `{}` characters.

## Exit codes

`0` success, `1` failure with a message on stderr. Nothing is retried
automatically.

## PGM files

The CLI accepts binary PGM so the engine can be exercised with real content
without an image decoder:

```bash
magick label.png -colorspace Gray -depth 8 -resize 812x1218 label.pgm
tspl-rs print --target "usb:…" --pgm label.pgm
```

The CLI does not scale: size the image to the label in dots first (812x1218
for 4x6 at 203 dpi). The app handles PDFs and any image size.
