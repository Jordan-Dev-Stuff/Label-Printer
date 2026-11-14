//! Where a job goes: a file, a serial port, a USB printer or a spooler queue.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::error::{Error, Result};

/// A parsed destination, from a `kind:value` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Write the job to a file instead of a printer.
    File(PathBuf),
    /// Write the job to a serial port or device node without framing.
    Serial(String),
    /// Write the job straight to a USB printer: a `usbprint.sys` interface
    /// path on Windows, a `/dev/usb/lp*` node on Linux.
    Usb(String),
    /// Submit the job to a spooler queue as a RAW job.
    Raw(String),
}

impl Target {
    /// The `kind` component of the spec.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::File(_) => "file",
            Self::Serial(_) => "serial",
            Self::Usb(_) => "usb",
            Self::Raw(_) => "raw",
        }
    }

    /// The `value` component of the spec.
    #[must_use]
    pub fn value(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::Serial(port) | Self::Usb(port) | Self::Raw(port) => port.clone(),
        }
    }

    /// Human-readable destination, e.g. `raw:BY-482BT-RAW`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{}:{}", self.kind(), self.value())
    }

    /// Delivers `job` to the destination.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyJob`] for an empty job, and the platform error
    /// when the file, port, device or queue cannot be written.
    pub fn send(&self, job: &[u8]) -> Result<()> {
        if job.is_empty() {
            return Err(Error::EmptyJob);
        }

        match self {
            Self::File(path) => send_file(path, job),
            Self::Serial(port) => send_serial(port, job),
            Self::Usb(path) => send_usb(path, job),
            Self::Raw(printer) => crate::spooler::send_raw(printer, job),
        }
    }
}

/// Parses `raw:PRINTER`, `usb:PATH`, `file:PATH` or `serial:PORT`.
///
/// The value is everything after the first colon, so Windows paths such as
/// `file:C:\temp\job.tspl` survive intact.
///
/// # Errors
///
/// Returns [`Error::InvalidTarget`] for a missing separator, an empty value or
/// an unknown kind.
pub fn parse_target(spec: &str) -> Result<Target> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(Error::InvalidTarget("empty spec".to_string()));
    }

    let Some((kind, value)) = spec.split_once(':') else {
        return Err(Error::InvalidTarget(format!(
            "{spec:?} must look like raw:PRINTER, usb:PATH, file:PATH or serial:PORT"
        )));
    };

    let kind = kind.trim().to_ascii_lowercase();
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::InvalidTarget(format!("{spec:?} has an empty value")));
    }

    match kind.as_str() {
        "raw" => Ok(Target::Raw(value.to_string())),
        "file" => Ok(Target::File(PathBuf::from(value))),
        "serial" => Ok(Target::Serial(value.to_string())),
        "usb" => Ok(Target::Usb(value.to_string())),
        other => Err(Error::InvalidTarget(format!(
            "unknown kind {other:?}, want raw, usb, file or serial"
        ))),
    }
}

/// Writes the job to `path`, creating missing parent directories.
fn send_file(path: &PathBuf, job: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| Error::io(format!("create {}", parent.display()), error))?;
        }
    }
    fs::write(path, job).map_err(|error| Error::io(format!("write {}", path.display()), error))
}

/// Writes the job straight to a serial port or device node.
fn send_serial(port: &str, job: &[u8]) -> Result<()> {
    let name = normalize_port(port);
    if name.is_empty() {
        return Err(Error::InvalidTarget("empty serial port".to_string()));
    }
    write_node(&name, job)
}

/// Writes the job to a USB printer interface.
fn send_usb(path: &str, job: &[u8]) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        return Err(Error::InvalidTarget("empty USB device path".to_string()));
    }
    #[cfg(windows)]
    {
        crate::os::write_device(path, job)
    }
    #[cfg(not(windows))]
    {
        write_node(path, job)
    }
}

/// Opens a device node for writing and writes everything.
fn write_node(name: &str, job: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(name)
        .map_err(|error| Error::io(format!("open {name:?}"), error))?;

    file.write_all(job)
        .map_err(|error| Error::io(format!("write {name:?}"), error))?;

    file.flush()
        .map_err(|error| Error::io(format!("flush {name:?}"), error))
}

/// Expands a bare COM name to the device namespace Windows requires above
/// COM9, and leaves everything else untouched.
#[must_use]
pub fn normalize_port(port: &str) -> String {
    let trimmed = port.trim();
    #[cfg(windows)]
    {
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with(r"\\.\") {
            return trimmed.to_string();
        }
        if upper.starts_with("COM") && upper.len() > 3 {
            return format!(r"\\.\{upper}");
        }
    }
    trimmed.to_string()
}
