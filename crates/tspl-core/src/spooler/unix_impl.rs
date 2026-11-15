//! Unix spooler access through `lp(1)`.
//!
//! `-o raw` disables CUPS filtering, so the device receives the job bytes
//! unmodified. A raw queue created with `lpadmin` is enough.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::spooler::QueueInfo;

/// Submits `job` to `printer` with CUPS filtering disabled.
///
/// # Errors
///
/// Returns [`Error::Spooler`] when `lp` reports a failure, [`Error::Io`] when
/// `lp` cannot be started, and [`Error::EmptyJob`] for an empty job.
pub fn send_raw(printer: &str, job: &[u8]) -> Result<()> {
    let printer = printer.trim();
    if printer.is_empty() {
        return Err(Error::InvalidTarget("empty printer name".to_string()));
    }
    if job.is_empty() {
        return Err(Error::EmptyJob);
    }

    let mut child = Command::new("lp")
        .args(["-d", printer, "-o", "raw", "-t", "Label Printer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::io("spawn lp", error))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(job)
            .map_err(|error| Error::io("write to lp stdin", error))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| Error::io("wait for lp", error))?;

    if !output.status.success() {
        return Err(Error::Spooler {
            printer: printer.to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(())
}

/// Lists CUPS queue names via `lpstat -e`.
///
/// # Errors
///
/// Returns [`Error::Io`] when `lpstat` is missing or fails.
pub fn list_queues() -> Result<Vec<String>> {
    let output = Command::new("lpstat")
        .arg("-e")
        .output()
        .map_err(|error| Error::io("run lpstat -e", error))?;

    if !output.status.success() {
        return Err(Error::Spooler {
            printer: "<lpstat>".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Lists queues with the device URI behind each one, from `lpstat -v`.
///
/// # Errors
///
/// Returns [`Error::Io`] when `lpstat` is missing or fails.
pub fn list_queue_details() -> Result<Vec<QueueInfo>> {
    let names = list_queues()?;

    // "device for BY482: usb://PEDOOLO/BY-482BT?serial=…". Missing or odd
    // output only costs the port column, never the queue list.
    let uris = Command::new("lpstat")
        .arg("-v")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();

    Ok(names
        .into_iter()
        .map(|name| {
            let prefix = format!("device for {name}:");
            let port = uris
                .lines()
                .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
                .map(|uri| uri.trim().to_string())
                .filter(|uri| !uri.is_empty());
            QueueInfo {
                name,
                port,
                driver: None,
            }
        })
        .collect())
}
