//! Error type shared by every module.

use std::fmt;

/// Errors produced while encoding a job, resolving a target or talking to a
/// printer.
#[derive(Debug)]
pub enum Error {
    /// A [`crate::Config`] value is outside the range a printer accepts.
    InvalidConfig(String),

    /// A frame has no pixels, or its buffer does not match its dimensions.
    InvalidFrame {
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },

    /// A `kind:value` target string could not be parsed.
    InvalidTarget(String),

    /// Refused to send a zero-length job.
    EmptyJob,

    /// An I/O operation failed. `context` names the file, port or command.
    Io {
        /// What was being attempted.
        context: String,
        /// The underlying operating-system error.
        source: std::io::Error,
    },

    /// The print spooler rejected the job.
    Spooler {
        /// Queue name the job was addressed to.
        printer: String,
        /// Human-readable cause from the operating system.
        message: String,
    },

    /// The operation does not exist on this platform.
    Unsupported(&'static str),
}

impl Error {
    /// Builds an [`Error::Io`] with context.
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// Builds an [`Error::Spooler`] from the last OS error.
    #[cfg(windows)]
    pub(crate) fn spooler(printer: &str, action: &str) -> Self {
        Self::Spooler {
            printer: printer.to_string(),
            message: format!("{action}: {}", std::io::Error::last_os_error()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid configuration: {message}"),
            Self::InvalidFrame { width, height } => write!(f, "invalid frame: {width}x{height}"),
            Self::InvalidTarget(message) => write!(f, "invalid target: {message}"),
            Self::EmptyJob => write!(f, "refusing to send an empty job"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Spooler { printer, message } => {
                write!(f, "spooler rejected job for {printer:?}: {message}")
            }
            Self::Unsupported(what) => write!(f, "unsupported on this platform: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
