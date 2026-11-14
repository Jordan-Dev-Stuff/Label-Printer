# Development

## Setup

1. Install Rust (the toolchain CI uses is pinned in
   `.github/workflows/ci.yml`; any version from 1.88 works locally).
2. Fetch the PDF renderer once per machine. `crates/thermal-ui/build.rs`
   embeds it into the app and stops the build with instructions if it is
   missing:

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
   ```

   ```bash
   ./scripts/fetch-pdfium.sh
   ```

   The library lands in `vendor/pdfium/<os>-<arch>/`, which is gitignored.

3. Linux only, system libraries for the window, file dialog and Bluetooth:

   ```bash
   sudo apt-get install -y pkg-config libdbus-1-dev libgtk-3-dev libxkbcommon-dev \
     libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
   ```

## Everyday commands

```bash
cargo run -p thermal-ui                   # the app
cargo run -p tspl-cli -- list             # the CLI
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
```

`cargo build --release` produces `target/release/thermal-ui(.exe)` and
`tspl-rs(.exe)`. The app exe is self-contained.

Useful app switches:

| Switch | Effect |
| --- | --- |
| `thermal-ui FILE` | Opens a PDF or image on start |
| `thermal-ui --render FILE [--page N] [--out PNG] [--width MM --height MM]` | Renders exactly what would print, no window; reports job sizes |
| `THERMAL_UI_SCREENSHOT=path.png` | Saves the window once it has settled, then exits (used for `docs/screenshots/`) |

A debug build keeps its settings in `target/debug/thermal-rs-data/` when
AppData is not writable, so it does not touch an installed copy's printer.

## The pdfium pin

`pdfium-render` binds to one pdfium API revision, selected by a Cargo
feature. Three places must name the same build; change them together:

| Where | Value |
| --- | --- |
| `crates/thermal-ui/Cargo.toml` | `pdfium-render = "=0.8.37"`, feature `pdfium_7543` |
| `crates/thermal-ui/src/pdf.rs` | `PDFIUM_RELEASE = "7543"` (also the unpack folder name) |
| `scripts/fetch-pdfium.ps1` and `.sh` | `chromium/7543` |

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main`, every pull request
and on demand, on `windows-latest` and `ubuntu-latest`:

1. Pinned Rust toolchain with clippy and rustfmt.
2. Linux system libraries (Linux only).
3. Fetch pdfium (the build needs it).
4. `cargo fmt --all --check`
5. `cargo clippy --all-targets -- -D warnings`
6. `cargo test --all`

## Releasing

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml`.
2. Commit, then tag and push:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

`.github/workflows/release.yml` checks that the tag matches `Cargo.toml`, runs
the tests, builds with `--locked`, builds the installer with Inno Setup and
publishes a GitHub Release with:

| File | Contents |
| --- | --- |
| `thermal-rs-<version>-setup.exe` | Per-user installer (no admin prompt) |
| `thermal-rs-<version>-portable.zip` | `thermal-ui.exe`, `tspl-rs.exe`, README, licenses |
| `SHA256SUMS.txt` | Checksums of both |

Running the workflow by hand (*Run workflow*, with a version such as
`0.0.0-test`) builds the same files as a workflow artifact without publishing
a release.

To build the installer locally, install Inno Setup 6, then:

```powershell
cargo build --release --locked
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /DAppVersion=0.2.0 installer\thermal-rs.iss
```

The installer is written to `target\installer\`.

## Conventions

- `tspl-core` keeps zero third-party dependencies and builds offline.
- `unsafe` lives only in `spooler/windows_impl.rs` and `os/windows.rs`.
- Any change to job bytes needs a test in `crates/tspl-core/tests/encode.rs`
  and a matching update to [protocol.md](protocol.md).
- Slow work never runs on the UI thread; see [ui.md](ui.md#threads).
