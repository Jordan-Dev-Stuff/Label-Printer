//! Bluetooth Classic serial (SPP) over a raw RFCOMM socket.
//!
//! Linux can open an RFCOMM channel to a remote address with a plain socket,
//! so printing over Bluetooth Classic needs no `rfcomm bind`, no
//! `/dev/rfcomm*` node and no root. Printers such as the BY-482BT accept the
//! connection without pairing, so there is nothing to set up beforehand.
//!
//! `socket` and `connect` are declared by hand, like the spooler bindings, so
//! the crate keeps zero dependencies. Other platforms reach Classic printers
//! through the serial port the system creates when pairing.

use crate::error::{Error, Result};

/// The SPP channel label printers use when none is given.
pub const DEFAULT_CHANNEL: u8 = 1;

/// Splits `DC:1D:30:87:F2:CE` or `DC:1D:30:87:F2:CE,2` into the address bytes
/// (as written, most significant first) and the RFCOMM channel.
///
/// # Errors
///
/// Returns [`Error::InvalidTarget`] for a malformed address or a channel
/// outside 1–30.
pub fn parse_spec(spec: &str) -> Result<([u8; 6], u8)> {
    let (address, channel) = match spec.trim().split_once(',') {
        Some((address, channel)) => {
            let channel = channel
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|c| (1..=30).contains(c));
            let Some(channel) = channel else {
                return Err(Error::InvalidTarget(format!(
                    "{spec:?}: the RFCOMM channel must be 1-30"
                )));
            };
            (address.trim(), channel)
        }
        None => (spec.trim(), DEFAULT_CHANNEL),
    };

    let parts: Vec<u8> = address
        .split(':')
        .filter(|part| part.len() == 2)
        .filter_map(|part| u8::from_str_radix(part, 16).ok())
        .collect();
    let Ok(bytes) = <[u8; 6]>::try_from(parts) else {
        return Err(Error::InvalidTarget(format!(
            "{address:?} is not a Bluetooth address like DC:1D:30:87:F2:CE"
        )));
    };
    Ok((bytes, channel))
}

/// Opens an RFCOMM connection to `spec` (see [`parse_spec`]). Writing to the
/// returned file sends bytes to the printer; dropping it closes the link
/// after queued data has gone out.
///
/// # Errors
///
/// Returns [`Error::InvalidTarget`] for a bad spec and [`Error::Io`] when the
/// printer cannot be reached (switched off, out of range, no SPP service).
#[cfg(target_os = "linux")]
pub fn connect(spec: &str) -> Result<std::fs::File> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::raw::{c_int, c_void};

    const AF_BLUETOOTH: u16 = 31;
    const SOCK_STREAM: c_int = 1;
    const SOCK_CLOEXEC: c_int = 0o2_000_000;
    const BTPROTO_RFCOMM: c_int = 3;

    /// `struct sockaddr_rc` from `<bluetooth/rfcomm.h>`.
    #[repr(C)]
    struct SockaddrRc {
        family: u16,
        /// Little-endian: the last byte of the written address comes first.
        bdaddr: [u8; 6],
        channel: u8,
    }

    extern "C" {
        fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
        fn connect(fd: c_int, address: *const c_void, len: u32) -> c_int;
    }

    let (mut bdaddr, channel) = parse_spec(spec)?;
    bdaddr.reverse();
    let address = SockaddrRc {
        family: AF_BLUETOOTH,
        bdaddr,
        channel,
    };

    // SAFETY: plain syscalls. The fd is owned by `OwnedFd` as soon as it is
    // valid, so every error path closes it, and `address` outlives the call.
    unsafe {
        let fd = socket(
            c_int::from(AF_BLUETOOTH),
            SOCK_STREAM | SOCK_CLOEXEC,
            BTPROTO_RFCOMM,
        );
        if fd < 0 {
            return Err(Error::io(
                "open a Bluetooth socket",
                std::io::Error::last_os_error(),
            ));
        }
        let fd = OwnedFd::from_raw_fd(fd);
        // 10 bytes; always fits.
        #[allow(clippy::cast_possible_truncation)]
        let len = std::mem::size_of::<SockaddrRc>() as u32;
        if connect(
            std::os::fd::AsRawFd::as_raw_fd(&fd),
            std::ptr::from_ref(&address).cast(),
            len,
        ) < 0
        {
            return Err(Error::io(
                format!("connect to {} over Bluetooth", spec.trim()),
                std::io::Error::last_os_error(),
            ));
        }
        Ok(std::fs::File::from(fd))
    }
}

/// RFCOMM sockets are Linux-only; elsewhere use the paired serial port.
///
/// # Errors
///
/// Always [`Error::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn connect(_spec: &str) -> Result<std::fs::File> {
    Err(Error::Unsupported(
        "rfcomm: targets; use the serial port created when pairing",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_with_and_without_channel() {
        let address = [0xDC, 0x1D, 0x30, 0x87, 0xF2, 0xCE];
        assert_eq!(parse_spec("DC:1D:30:87:F2:CE").unwrap(), (address, 1));
        assert_eq!(parse_spec(" dc:1d:30:87:f2:ce , 3 ").unwrap(), (address, 3));
    }

    #[test]
    fn bad_specs_are_rejected() {
        for spec in [
            "",
            "DC:1D:30:87:F2",
            "DC:1D:30:87:F2:CE:00",
            "DC:1D:30:87:F2:ZZ",
            "DC1D3087F2CE",
            "DC:1D:30:87:F2:CE,0",
            "DC:1D:30:87:F2:CE,31",
            "DC:1D:30:87:F2:CE,x",
        ] {
            assert!(parse_spec(spec).is_err(), "{spec:?} should be rejected");
        }
    }
}
