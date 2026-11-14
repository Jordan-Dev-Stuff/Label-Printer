//! Windows spooler access through `winspool.drv`.
//!
//! A job submitted with the `RAW` datatype bypasses the print processor and
//! the driver's rendering code, so the port receives our TSPL bytes exactly as
//! written. That is why an inbox *Generic / Text Only* queue is enough and no
//! vendor driver is required.

use std::ffi::c_void;
use std::ptr;

use crate::error::{Error, Result};
use crate::spooler::QueueInfo;

/// `PRINTER_ENUM_LOCAL`
const PRINTER_ENUM_LOCAL: u32 = 0x0000_0002;
/// `PRINTER_ENUM_CONNECTIONS`
const PRINTER_ENUM_CONNECTIONS: u32 = 0x0000_0004;

type Handle = *mut c_void;

#[repr(C)]
struct DocInfo1 {
    doc_name: *const u16,
    output_file: *const u16,
    data_type: *const u16,
}

#[repr(C)]
struct PrinterInfo2 {
    server_name: *const u16,
    printer_name: *const u16,
    share_name: *const u16,
    port_name: *const u16,
    driver_name: *const u16,
    comment: *const u16,
    location: *const u16,
    dev_mode: *const c_void,
    sep_file: *const u16,
    print_processor: *const u16,
    data_type: *const u16,
    parameters: *const u16,
    security_descriptor: *const c_void,
    attributes: u32,
    priority: u32,
    default_priority: u32,
    start_time: u32,
    until_time: u32,
    status: u32,
    jobs: u32,
    average_ppm: u32,
}

#[allow(non_snake_case)]
#[link(name = "Winspool")]
unsafe extern "system" {
    fn OpenPrinterW(printer_name: *const u16, printer: *mut Handle, defaults: *const c_void)
        -> i32;
    fn ClosePrinter(printer: Handle) -> i32;
    fn StartDocPrinterW(printer: Handle, level: u32, info: *const DocInfo1) -> u32;
    fn StartPagePrinter(printer: Handle) -> i32;
    fn WritePrinter(printer: Handle, buffer: *const c_void, len: u32, written: *mut u32) -> i32;
    fn EndPagePrinter(printer: Handle) -> i32;
    fn EndDocPrinter(printer: Handle) -> i32;
    fn EnumPrintersW(
        flags: u32,
        name: *const u16,
        level: u32,
        buffer: *mut u8,
        size: u32,
        needed: *mut u32,
        returned: *mut u32,
    ) -> i32;
}

/// Closes the printer handle when dropped, including on early return.
struct HandleGuard(Handle);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            ClosePrinter(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a NUL-terminated UTF-16 string.
///
/// # Safety
///
/// `ptr` must point to a readable NUL-terminated UTF-16 buffer.
unsafe fn wide_to_string(ptr: *const u16) -> String {
    let mut len = 0_usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}

/// Submits `job` to `printer` as a RAW job.
///
/// # Errors
///
/// Returns [`Error::Spooler`] when the queue cannot be opened or the job is
/// rejected; [`Error::EmptyJob`] for an empty job.
pub fn send_raw(printer: &str, job: &[u8]) -> Result<()> {
    let printer = printer.trim();
    if printer.is_empty() {
        return Err(Error::InvalidTarget("empty printer name".to_string()));
    }
    if job.is_empty() {
        return Err(Error::EmptyJob);
    }
    if u32::try_from(job.len()).is_err() {
        return Err(Error::Spooler {
            printer: printer.to_string(),
            message: format!("job of {} bytes exceeds the 4 GiB RAW limit", job.len()),
        });
    }

    let name = wide(printer);
    let mut raw_handle: Handle = ptr::null_mut();
    if unsafe { OpenPrinterW(name.as_ptr(), &raw mut raw_handle, ptr::null()) } == 0 {
        return Err(Error::spooler(printer, "OpenPrinter"));
    }
    let guard = HandleGuard(raw_handle);

    let doc_name = wide("thermal-rs");
    let data_type = wide("RAW");
    let info = DocInfo1 {
        doc_name: doc_name.as_ptr(),
        output_file: ptr::null(),
        data_type: data_type.as_ptr(),
    };

    if unsafe { StartDocPrinterW(guard.0, 1, &raw const info) } == 0 {
        return Err(Error::spooler(printer, "StartDocPrinter"));
    }

    if unsafe { StartPagePrinter(guard.0) } == 0 {
        unsafe {
            EndDocPrinter(guard.0);
        }
        return Err(Error::spooler(printer, "StartPagePrinter"));
    }

    let mut written: u32 = 0;
    let write_ok = unsafe {
        WritePrinter(
            guard.0,
            job.as_ptr().cast(),
            u32::try_from(job.len()).unwrap_or(u32::MAX),
            &raw mut written,
        )
    } != 0;

    unsafe {
        EndPagePrinter(guard.0);
        EndDocPrinter(guard.0);
    }

    if !write_ok {
        return Err(Error::spooler(printer, "WritePrinter"));
    }
    if written as usize != job.len() {
        return Err(Error::Spooler {
            printer: printer.to_string(),
            message: format!("wrote {written} of {} bytes", job.len()),
        });
    }

    Ok(())
}

/// Lists local and connection printer queue names.
///
/// # Errors
///
/// Returns [`Error::Spooler`] when the spooler enumeration fails.
pub fn list_queues() -> Result<Vec<String>> {
    Ok(list_queue_details()?
        .into_iter()
        .map(|queue| queue.name)
        .collect())
}

/// Lists queues with the port and driver behind each one.
///
/// # Errors
///
/// Returns [`Error::Spooler`] when the spooler enumeration fails.
pub fn list_queue_details() -> Result<Vec<QueueInfo>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed: u32 = 0;
    let mut returned: u32 = 0;

    unsafe {
        EnumPrintersW(
            flags,
            ptr::null(),
            2,
            ptr::null_mut(),
            0,
            &raw mut needed,
            &raw mut returned,
        );
    }

    if needed == 0 {
        return Ok(Vec::new());
    }

    // The buffer holds the structs followed by the strings they point into, so
    // it must outlive the reads below. `u64` elements keep it pointer-aligned.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    let mut needed_again: u32 = 0;
    let mut returned_again: u32 = 0;
    let ok = unsafe {
        EnumPrintersW(
            flags,
            ptr::null(),
            2,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed_again,
            &raw mut returned_again,
        )
    };

    if ok == 0 {
        return Err(Error::spooler("<enum>", "EnumPrinters"));
    }

    let stride = std::mem::size_of::<PrinterInfo2>();
    let base: *const PrinterInfo2 = buffer.as_ptr().cast();
    let optional = |text: *const u16| {
        (!text.is_null())
            .then(|| unsafe { wide_to_string(text) })
            .filter(|value| !value.is_empty())
    };

    let mut queues = Vec::new();
    for index in 0..returned_again as usize {
        if (index + 1) * stride > needed as usize {
            break;
        }
        let info = unsafe { ptr::read(base.add(index)) };
        if info.printer_name.is_null() {
            continue;
        }
        queues.push(QueueInfo {
            name: unsafe { wide_to_string(info.printer_name) },
            port: optional(info.port_name),
            driver: optional(info.driver_name),
        });
    }

    queues.sort_by(|a, b| a.name.cmp(&b.name));
    queues.dedup_by(|a, b| a.name == b.name);
    Ok(queues)
}
