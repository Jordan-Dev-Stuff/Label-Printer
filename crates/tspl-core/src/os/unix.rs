//! Unix device enumeration: USB printer nodes and serial ports under `/dev`.
//!
//! Linux exposes USB printer-class devices as `/dev/usb/lpN` through the
//! `usblp` driver, and their IEEE 1284 id in sysfs, so a label printer is
//! found and identified with no CUPS queue at all. macOS has no such node and
//! relies on CUPS queues or `/dev/cu.*` serial ports.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// A USB printer device node.
#[derive(Debug, Clone)]
pub struct UsbPrinterInfo {
    /// `/dev/usb/lp0`.
    pub path: String,
    /// IEEE 1284 id from sysfs, when the kernel exposes it.
    pub device_id: Option<String>,
}

/// Lists `/dev/usb/lp*` nodes. Missing `/dev/usb` is a normal state.
///
/// # Errors
///
/// Returns [`Error::Io`] when `/dev/usb` exists but cannot be read.
pub fn usb_printers() -> Result<Vec<UsbPrinterInfo>> {
    let dir = Path::new("/dev/usb");
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(dir).map_err(|error| Error::io("read /dev/usb", error))?;
    let mut printers: Vec<UsbPrinterInfo> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("lp"))
        .map(|name| {
            let device_id =
                fs::read_to_string(format!("/sys/class/usbmisc/{name}/device/ieee1284_id"))
                    .ok()
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty());
            UsbPrinterInfo {
                path: format!("/dev/usb/{name}"),
                device_id,
            }
        })
        .collect();

    printers.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(printers)
}

/// Lists serial device nodes worth offering.
///
/// macOS creates a `tty.*` and a `cu.*` node per port; only `cu.*` is useful
/// for an outgoing connection, so `tty.*` is skipped, as are the always-present
/// console and incoming-Bluetooth nodes.
///
/// # Errors
///
/// Returns [`Error::Io`] when `/dev` cannot be read.
pub fn serial_ports() -> Result<Vec<String>> {
    let entries = fs::read_dir("/dev").map_err(|error| Error::io("read /dev", error))?;

    let mut ports: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| crate::discover::is_useful_serial_node(name))
        .map(|name| format!("/dev/{name}"))
        .collect();

    ports.sort();
    ports.dedup();
    Ok(ports)
}
