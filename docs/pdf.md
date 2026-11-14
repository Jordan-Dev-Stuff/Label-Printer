# PDF support

PDFs are a *source* format: they are rasterised on the host and then encoded
as TSPL like any image. `tspl-core` is untouched; it still receives a `Frame`.
The renderer lives in the UI ([`crates/thermal-ui/src/pdf.rs`](../crates/thermal-ui/src/pdf.rs)), the
fitting maths in [`crates/thermal-ui/src/content.rs`](../crates/thermal-ui/src/content.rs).

## Nothing to install

pdfium is **compiled into `thermal-ui.exe`**. On first use the app writes the
library to `%LOCALAPPDATA%\thermal-rs\pdfium-7543\` (falling back to the
exe's folder, then the temp folder) and loads it from there. Users never
download anything, and the installer ships a single executable.

Developers fetch the library once per build machine; `crates/thermal-ui/build.rs` embeds it
and refuses to build without it:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
cargo build --release
```

```bash
./scripts/fetch-pdfium.sh
cargo build --release
```

The file lands in `vendor/pdfium/<os>-<arch>/` (gitignored).
`-ExecutionPolicy Bypass` applies to that one PowerShell process only; Windows
client editions refuse every `.ps1` file by default.

## Why pdfium, and why version 7543

| Option | Verdict |
| --- | --- |
| **pdfium** (via `pdfium-render`) | Chosen: BSD-3, prebuilt for every platform, renders what Chrome and Edge show |
| MuPDF | AGPL, and the Rust bindings need a C toolchain |
| Poppler | GPL, and must be installed on every machine |
| `lopdf` and other pure-Rust parsers | Parse only; no renderer |
| `Windows.Data.Pdf` | Windows-only and pulls in the `windows` crate |

`pdfium-render` binds to one pdfium API revision, chosen by a Cargo feature.
The first UI used the default (`pdfium_latest` = 7543) while the fetch script
downloaded *latest* pdfium (8152 at the time), so the bindings and the library
disagreed. Now `crates/thermal-ui/Cargo.toml` pins `pdfium-render = "=0.8.37"` with
`pdfium_7543`, and both fetch scripts download `chromium/7543`. Change all three
together.

## How a page is rendered

```text
PDF ─probe 72 dpi─► content box ─plan─► render at exact scale ─crop─► turn ─► Frame ─► halftone ─► TSPL
```

1. **Probe.** Render the page at 72 dpi and find the bounding box of anything
   darker than near-white (plus a 1% margin).
2. **Plan** ([`plan_pdf`](../crates/thermal-ui/src/content.rs)). Decide whether to turn the
   content to match the label, then the scale that makes the *content*, not
   the page, fill the label.
3. **Render once** at that scale, crop to the content, turn it.
4. **Place** it centred on a white label-sized frame. Results within two dots
   of the label size are not resampled at all.
5. **Halftone.** *Auto* thresholds line art (text, barcodes) and dithers
   photos. Dithering a barcode makes its bar edges ragged.

The probe is what fixes shipping labels: a 4×6 label in the corner of a
Letter page used to be rendered at 812 dots for the *whole page*, then cropped
and enlarged about twofold, which blurred barcodes. It is now rendered at
exactly 203 dpi.

pdfium is initialised once per process, on the render worker thread. The first
UI created a new `Pdfium` (`FPDF_InitLibrary` / `FPDF_DestroyLibrary`) for
every page count and every preview, on the UI thread, which pdfium does not
support reliably.

## Checking a PDF without the window

```powershell
.\target\release\thermal-ui.exe --render label.pdf --page 1 --out preview.png
```

It writes exactly what would print and reports the size, black coverage and
page count. `crates/thermal-ui/tests/fixtures/letter-with-4x6-label.pdf` is the regression
case: a 4×6 label in the corner of a Letter page.

## Limitations

- **Form fields** print as pdfium draws them, with their values.
- **Fonts must be embedded**, or pdfium substitutes them and metrics shift.
- **Vector precision is limited by 203 dpi.** Lines thinner than one dot
  (0.125 mm) may drop out.
- **Password-protected PDFs** are refused with a message.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Build fails: `pdfium … is missing` | Library not fetched on this build machine | Run `scripts/fetch-pdfium.ps1` |
| `could not unpack pdfium` | No writable data, exe or temp folder | Check permissions on `%LOCALAPPDATA%` |
| `this build has no PDF support` | Built with `--no-default-features` | Rebuild with default features |
| Content too small | Trim blank margins is off, or the page has marks near its edges | Turn on *Trim blank margins* |
| Barcode looks speckled | Halftone set to *Photo* | Use *Auto* or *Text & barcodes* |
