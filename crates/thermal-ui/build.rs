//! Embeds the pinned pdfium shared library into the executable, so the app
//! (and its installer) needs nothing else at runtime.
//!
//! The library comes from `vendor/pdfium/<os>-<arch>/` at the repository root,
//! filled once per build machine by `scripts/fetch-pdfium.ps1` / `.sh`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(pdfium_embedded)");
    if std::env::var_os("CARGO_FEATURE_PDF").is_none() {
        return;
    }

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let (dir_os, library) = match os.as_str() {
        "windows" => ("windows", "pdfium.dll"),
        "macos" => ("macos", "libpdfium.dylib"),
        _ => ("linux", "libpdfium.so"),
    };

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    // crates/thermal-ui -> repository root.
    let root = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap_or(&manifest);
    let path = root
        .join("vendor")
        .join("pdfium")
        .join(format!("{dir_os}-{arch}"))
        .join(library);
    println!("cargo:rerun-if-changed={}", path.display());

    if path.is_file() {
        println!("cargo:rustc-env=THERMAL_PDFIUM_LIBRARY={}", path.display());
        println!("cargo:rustc-cfg=pdfium_embedded");
    } else {
        let script = if os == "windows" {
            r"powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1"
        } else {
            "./scripts/fetch-pdfium.sh"
        };
        panic!(
            "\n\nPDF support embeds pdfium, but {} is missing.\n\
             Fetch it once (from the repository root):\n    {script}\n\
             or build without PDF support: cargo build --no-default-features\n\n",
            path.display()
        );
    }
}
