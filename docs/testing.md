# Testing

Three layers, in order of how fast they fail.

## 1. Automated tests (no hardware)

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

CI runs exactly these on Windows and Linux for every push and pull request
(see [development.md](development.md)).

### Engine: `crates/tspl-core`

| Test | Proves |
| --- | --- |
| `white_4x6_job_matches_reference_layout` | Exact 124,371-byte job, command order, header text, all-`0xFF` white raster, tail |
| `packing_is_msb_first_with_black_as_zero` | Bit order, polarity (cleared bit = black) and white padding |
| `compact_jobs_send_only_inked_bands` | Byte-aligned `BITMAP` bands around separate blocks, small job |
| `compact_bands_merge_short_gaps_and_keep_polarity` | Short blank runs stay in a band and stay white |
| `compact_blank_label_has_no_bitmap` | A blank label is `CLS` then `PRINT` |
| `size_is_derived_from_the_frame_when_unset` | Millimetres from dots |
| `copies_gap_and_settings_are_honoured` | `PRINT n,1`, `GAP 0`, explicit `SIZE` |
| `bad_configuration_is_rejected` | Out-of-range settings name their field |
| `frame_*`, `dither_*`, `test_card_*` | Bounds, halftone behaviour, determinism |
| `targets_*` | `raw:`, `usb:`, `serial:`, `file:` parsing, drive letters, junk rejection |
| `file_target_writes_the_job_and_creates_directories` | Real file round trip |
| `queue_enumeration_succeeds`, `raw_send_to_an_unknown_queue_…` | Spooler access on this host |

The 124,371-byte job is the anchor: it is the 4x6 job a BY-482BT printed, so
any change to command order, number formatting or padding fails before it
reaches paper.

`discover.rs` unit tests cover discovery with recorded facts instead of
hardware: a queue linked to a present USB printer (online, hidden as a
duplicate of direct USB), the same queue with the printer unplugged
(offline), direct USB with no queue, a paired Bluetooth printer named from the
Bluetooth stack, earbuds on a Bluetooth serial port (hidden), incoming-only
Bluetooth ports (dropped), IEEE 1284 id parsing and reply decoding, and that a
non-label office printer is never chosen automatically.

### App: `crates/thermal-ui`

| Module | Proves |
| --- | --- |
| `content` | Crop, rotation, fit, placement, halftone choice, and the PDF render plan (a 4×6 label on Letter renders at exactly 203 dpi; huge pages are capped) |
| `jobs` | Test-card preview, missing or corrupt files name themselves, a print to `file:` writes one job, Bluetooth jobs come back to the UI |
| `ble` | Print channel choice (known printer UUIDs first, never the writable device name), write modes, finding the remembered printer by id or name |
| `settings` | Presets, range repair, old settings files still load, storage falls back to a writable folder and keeps existing settings where they are |
| `app` | Time-left estimate, reconnect back-off |

### PDF end to end

With the embedded pdfium, render the regression fixture (a 4×6 label in the
corner of a Letter page) exactly as it would print:

```powershell
.\target\release\thermal-ui.exe --render crates\thermal-ui\tests\fixtures\letter-with-4x6-label.pdf --out preview.png
```

Expect `812x1218 dots … pages: 1` and a crisp, cropped label in the PNG, like
[the README screenshot](screenshots/label-output.png).

## 2. Byte-level check without a printer

Encode to a file and inspect it:

```powershell
.\target\release\tspl-rs.exe print --target file:out\card.tspl --pattern testcard
(Get-Item out\card.tspl).Length          # 124371 for a 4x6 label
Get-Content out\card.tspl -TotalCount 1  # SIZE 101.60 mm,152.40 mm
```

To prove a print queue delivers RAW bytes untouched, give a queue a file port
and compare (elevated, once):

```powershell
Add-PrinterPort -Name "$PWD\out\capture.prn"
Add-Printer -Name "LABEL-CAPTURE" -DriverName "Generic / Text Only" -PortName "$PWD\out\capture.prn"

.\target\release\tspl-rs.exe print --target raw:LABEL-CAPTURE --pattern testcard
fc.exe /b out\capture.prn out\card.tspl
```

`FC: no differences encountered` means the queue is a pure byte pipe.

## 3. Hardware acceptance

1. `tspl-rs list` shows the printer as `USB` (no queue needed).
2. Print the test card over USB; check orientation, scale, the black border and
   corner squares, and the gradient. **The border must be black on white**; a
   black label with white marks means the bitmap polarity is inverted.
3. Unplug USB, connect over Bluetooth LE in the app, print again. Then close
   and reopen the app: it must reconnect by itself.
4. Print a shipping-label PDF and scan its barcode with a phone.
5. Change darkness and speed once each and confirm the difference is visible.

| Observation | Meaning |
| --- | --- |
| Whole label black, content white | Bitmap polarity inverted (see [protocol.md](protocol.md#bitmap-xywidthheightmodedata)) |
| Gradient bands smoothly | Dithering and darkness are right |
| Checker is grey mush | Too much heat or too slow; raise speed or lower darkness |
| Content shifted vertically | `SIZE`/`GAP` disagree with the media; **Calibrate** |
| Every other label blank | Gap too large; **Calibrate** |
| Bluetooth label garbled or cut short | *Faster sending* overran the printer; turn it off |

## What is not tested automatically

- Writes to a real printer over USB or Bluetooth LE (needs the hardware).
- The Unix device code on real Linux and macOS hardware (CI compiles and unit
  tests it on Linux).
- CUPS behaviour (`lp -o raw`; testable only with a stub `lp` on `PATH`).
- Print quality, which is inherently visual.
