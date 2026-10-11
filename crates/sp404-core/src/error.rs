use std::fmt;

/// Recoverable failure from the sampler core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No effect is registered under `id`.
    UnknownEffect {
        /// The id that was requested.
        id: String,
    },
    /// `effect` has no parameter named `param`.
    UnknownParam {
        /// Effect id.
        effect: String,
        /// Parameter id.
        param: String,
    },
    /// A parameter value was rejected before clamping could save it.
    InvalidValue {
        /// Effect id.
        effect: String,
        /// Parameter id.
        param: String,
        /// Why the value was rejected.
        reason: String,
    },
    /// Pad index is outside the bank.
    PadOutOfRange {
        /// The rejected index.
        index: usize,
    },
    /// Effect slot index is outside the chain.
    EffectOutOfRange {
        /// The rejected index.
        index: usize,
    },
    /// Chain or catalog JSON could not be parsed.
    Json(String),
    /// WAV bytes were not a supported PCM or float file.
    Wav(String),
    /// Buffer had no frames or an unsupported layout.
    InvalidBuffer(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownEffect { id } => write!(f, "unknown effect '{id}'"),
            Error::UnknownParam { effect, param } => {
                write!(f, "effect '{effect}' has no parameter '{param}'")
            }
            Error::InvalidValue {
                effect,
                param,
                reason,
            } => write!(f, "invalid value for {effect}.{param}: {reason}"),
            Error::PadOutOfRange { index } => write!(f, "pad index {index} is out of range"),
            Error::EffectOutOfRange { index } => {
                write!(f, "effect index {index} is out of range")
            }
            Error::Json(msg) => write!(f, "invalid chain JSON: {msg}"),
            Error::Wav(msg) => write!(f, "wav: {msg}"),
            Error::InvalidBuffer(msg) => write!(f, "invalid audio buffer: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
