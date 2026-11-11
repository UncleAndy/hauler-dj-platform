/// Errors that can occur during fingerprinting.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid sample rate: {0} (must be > 1000)")]
    InvalidSampleRate(u32),

    #[error("invalid channel count: {0} (must be >= 1)")]
    InvalidChannelCount(u16),

    #[error("unknown algorithm: {0}")]
    UnknownAlgorithm(u8),

    #[error("resampling not available (enable 'resample' feature)")]
    ResamplingUnavailable,

    #[error("invalid fingerprint data")]
    InvalidFingerprint,

    #[error("pipeline not started (call start() first)")]
    NotStarted,

    #[error("resampling error: {0}")]
    Resample(String),
}

pub type Result<T> = std::result::Result<T, Error>;
