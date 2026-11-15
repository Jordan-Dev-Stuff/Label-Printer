//! Driverless printing for TSPL label printers.
//!
//! This crate is the engine behind Label Printer. It takes an 8-bit grayscale
//! [`Frame`], halftones it, encodes a TSPL/TSPL2 job and delivers it to a file,
//! a serial port or a spooler queue — no vendor driver in the path.
//!
//! ```no_run
//! use tspl_core::{encode, parse_target, Config, Frame};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let frame = Frame::test_card(812, 1218)?;
//! let frame = frame.dither();
//! let job = encode(&frame, &Config::by482())?;
//! parse_target("raw:BY-482BT-RAW")?.send(&job)?;
//! # Ok(())
//! # }
//! ```
//!
//! Design notes live in the repository `docs/` directory, in particular
//! `docs/protocol.md` for the byte layout and `docs/transports.md` for why RAW
//! delivery needs no driver.

#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod error;
mod os;
mod spooler;

pub mod discover;
pub mod encode;
pub mod frame;
pub mod target;

pub use encode::{
    calibrate_job, encode, feed_job, px_to_mm, Config, DEFAULT_DPI, DEFAULT_GAP_MM,
    MAX_HEAD_WIDTH_MM,
};
pub use error::{Error, Result};
pub use frame::Frame;
pub use target::{normalize_port, parse_target, Target};
