//! Platform device access beyond the spooler: USB printer interfaces and
//! serial ports. One implementation per platform family, chosen by `cfg`.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{serial_ports, usb_printers, write_device};

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::{serial_ports, usb_printers};
