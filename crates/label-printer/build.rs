//! Embeds the pinned pdfium shared library into the executable, so the app
//! (and its installer) needs nothing else at runtime.
//!
//! The library comes from `vendor/pdfium/<os>-<arch>/` at the repository root,
//! filled once per build machine by `scripts/fetch-pdfium.ps1` / `.sh`.

use std::path::PathBuf;

fn main() {
    embed_windows_resources();
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
    // crates/label-printer -> repository root.
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
        println!(
            "cargo:rustc-env=LABEL_PRINTER_PDFIUM_LIBRARY={}",
            path.display()
        );
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

/// Windows: embed the app icon and version info in the .exe, so Explorer,
/// the taskbar and shortcuts show the Label Printer icon. Never fails the
/// build: without the Windows SDK's resource compiler the exe just keeps the
/// default icon.
fn embed_windows_resources() {
    println!("cargo:rerun-if-changed=../../assets/label-printer.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../../assets/label-printer.ico")
        .set("ProductName", "Label Printer")
        .set("FileDescription", "Label Printer")
        .set("CompanyName", "Label Printer");
    if let Err(error) = resource.compile() {
        println!(
            "cargo:warning=couldn't embed the Windows icon ({error}); the exe keeps the default icon"
        );
    }
}
