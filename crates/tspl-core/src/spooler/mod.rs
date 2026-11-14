//! Spooler access: one implementation per platform family.
//!
//! Windows submits RAW jobs through `winspool.drv`; Unix shells out to
//! `lp -o raw`. Both deliver the job bytes unmodified, which is what lets a
//! driverless queue print TSPL directly.

/// A spooler queue and what sits behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueInfo {
    /// Queue name, as passed to `raw:`.
    pub name: String,
    /// Port (`USB001`, `PORTPROMPT:`) on Windows, device URI on CUPS.
    pub port: Option<String>,
    /// Driver name on Windows, e.g. `Generic / Text Only`.
    pub driver: Option<String>,
}

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::{list_queue_details, list_queues, send_raw};

#[cfg(not(windows))]
mod unix_impl;
#[cfg(not(windows))]
pub use unix_impl::{list_queue_details, list_queues, send_raw};
