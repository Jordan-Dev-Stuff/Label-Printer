# Development

## Setup

1. Install Rust (the toolchain CI uses is pinned in
   `.github/workflows/ci.yml`; any version from 1.88 works locally).
2. Fetch the PDF renderer once per machine. `crates/label-printer/build.rs`
   embeds it into the app and stops the build with instructions if it is
   missing:

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
   ```

   ```bash
   ./scripts/fetch-pdfium.sh
   ```

   The library lands in `vendor/pdfium/<os>-<arch>/`, which is gitignored.

3. Linux only, system libraries for the window and Bluetooth:

   ```bash
   sudo apt-get install -y pkg-config libdbus-1-dev libxkbcommon-dev \
     libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
   ```

## Everyday commands

```bash
cargo run -p label-printer                # the app
cargo run -p tspl-cli -- list             # the CLI
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
```

`cargo build --release` produces `target/release/label-printer(.exe)` and
`tspl-rs(.exe)`. The app exe is self-contained.

Useful app switches:

| Switch | Effect |
| --- | --- |
| `label-printer FILE` | Opens a PDF or image on start |
| `label-printer --render FILE [--page N] [--out PNG] [--width MM --height MM]` | Renders exactly what would print, no window; reports job sizes |
| `LABEL_PRINTER_SCREENSHOT=path.png` | Saves the window once it has settled, then exits (used for `docs/screenshots/`) |

A debug build keeps its settings in `target/debug/label-printer-data/` when
AppData is not writable, so it does not touch an installed copy's printer.

## The pdfium pin

`pdfium-render` binds to one pdfium API revision, selected by a Cargo
feature. Three places must name the same build; change them together:

| Where | Value |
| --- | --- |
| `crates/label-printer/Cargo.toml` | `pdfium-render = "=0.8.37"`, feature `pdfium_7543` |
| `crates/label-printer/src/pdf.rs` | `PDFIUM_RELEASE = "7543"` (also the unpack folder name) |
| `scripts/fetch-pdfium.ps1` and `.sh` | `chromium/7543` |

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main`, every pull request
and on demand, on `windows-latest`, `ubuntu-latest` and `macos-latest`:

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
   git tag v0.3.1
   git push origin v0.3.1
   ```

`.github/workflows/release.yml` checks that the tag matches `Cargo.toml`, then
builds on three runners in parallel (each runs the tests and builds with
`--locked`), and publishes one GitHub Release only if all three succeed:

| File | Built on | Contents |
| --- | --- | --- |
| `Label-Printer-<version>-setup.exe` | `windows-latest` | Per-user installer (no admin prompt), Inno Setup |
| `Label-Printer-<version>-windows-x64-portable.zip` | `windows-latest` | `label-printer.exe`, `tspl-rs.exe`, README, licenses |
| `Label-Printer-<version>-linux-amd64.deb` | `ubuntu-22.04` | Both programs, menu entry, icon, USB udev rule; dependencies declared |
| `Label-Printer-<version>-linux-x86_64.AppImage` | `ubuntu-22.04` | Both programs in one executable file, any distro |
| `Label-Printer-<version>-macos-universal.zip` | `macos-latest` | `Label Printer.app` (Apple Silicon + Intel), `tspl-rs` |
| `SHA256SUMS.txt` | | Checksums of all of the above |

Platform notes:

- **Windows:** the icon and version info are embedded in the exe by
  `crates/label-printer/build.rs` (`winresource`), from
  `assets/label-printer.ico`. Shortcuts take their icon from the exe.
- **Linux:** built on Ubuntu 22.04 so it needs only glibc 2.35. Runtime
  needs D-Bus, xkbcommon and EGL/OpenGL, plus xdg-desktop-portal for the
  file dialog (all present on desktop distributions; GTK is not used).
  - The `.deb` is made by `cargo-deb` from `[package.metadata.deb]` in
    `crates/label-printer/Cargo.toml`. Its dependencies are worked out from
    the binaries' shared libraries. It installs
    `packaging/linux/70-label-printer.rules`, which tags the printer's
    `/dev/usb/lpN` with `uaccess` so the logged-in user can print without
    root or the `lp` group; `packaging/linux/debian/postinst` reloads udev
    so it applies to a printer that is already plugged in.
  - The AppImage is made by `appimagetool` (pinned to 1.9.1) with
    `--comp gzip`: the default zstd cannot be read by the libappimage in
    AppImageLauncher and similar menu-integration tools ("Failed to register
    AppImage in system via libappimage"). The release job checks the
    compression and the root desktop entry and icon. The runtime is
    static, so users do not need FUSE 2. System libraries are not bundled; they
    come from the host, like the plain binary.
  - Before uploading, the job runs the packaged `label-printer --render` on
    the regression PDF from both the `.deb` and the AppImage, and checks that
    `tspl-rs` writes the reference 124,371-byte job.
- **macOS:** each architecture embeds its own pdfium, then `lipo` joins them.
  The `.icns` is made by `iconutil` from the PNGs the app exports
  (`--export-icons`). `Info.plist` carries the Bluetooth permission text,
  without which macOS terminates the app on first Bluetooth use. The bundle
  is ad-hoc signed, not notarised: first launch needs right-click → Open.

### The icon

The icon is drawn in code (`crates/label-printer/src/icon.rs`). After changing
it, regenerate the committed files:

```bash
cargo run -p label-printer -- --export-icons target/icons
cp target/icons/label-printer.ico assets/label-printer.ico
cp target/icons/label-printer-256.png assets/label-printer.png
```

Running the workflow by hand (*Run workflow*, with a version such as
`0.0.0-test`) builds the same files as a workflow artifact without publishing
a release.

To build the installer locally, install Inno Setup 6, then:

```powershell
cargo build --release --locked
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /DAppVersion=0.3.1 installer\label-printer.iss
```

The installer is written to `target\installer\`.

## Conventions

- `tspl-core` keeps zero third-party dependencies and builds offline.
- `unsafe` lives only in `spooler/windows_impl.rs` and `os/windows.rs`.
- Any change to job bytes needs a test in `crates/tspl-core/tests/encode.rs`
  and a matching update to [protocol.md](protocol.md).
- Slow work never runs on the UI thread; see [ui.md](ui.md#threads).
