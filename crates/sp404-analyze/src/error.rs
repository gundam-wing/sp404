use std::fmt;

/// Recoverable failure from analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The sample buffer cannot be analyzed.
    InvalidInput(String),
    /// A caller option is outside the range the detectors accept.
    InvalidOption(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidInput(msg) | Error::InvalidOption(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}
