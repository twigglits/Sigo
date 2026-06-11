//! Sigo error types. [`SigoError`] is a thiserror enum covering translator,
//! backend, tokenizer, config, eval, and I/O errors. [`Result`] is a shorthand
//! alias defaulting to [`SigoError`].

use thiserror::Error;

/// Convenience alias defaulting to [`SigoError`].
pub type Result<T, E = SigoError> = std::result::Result<T, E>;

/// Errors that can occur during translation, backend calls, or benchmarking.
#[derive(Debug, Error)]
pub enum SigoError {
    /// The local translator (Ollama) returned an error.
    #[error("translator error: {0}")]
    Translator(String),

    /// The translator request timed out.
    #[error("translator timed out after {0:?}")]
    TranslatorTimeout(std::time::Duration),

    /// The Claude backend returned an error.
    #[error("claude backend error: {0}")]
    Backend(String),

    /// The Claude stream disconnected before the `Done` event.
    #[error("claude stream disconnected mid-turn after {bytes_received} bytes")]
    StreamDisconnect {
        /// Number of bytes received before the disconnect.
        bytes_received: usize,
    },

    /// Claude authentication failed (401/403).
    #[error("claude auth failed: {0}")]
    Auth(String),

    /// Rate limited (429) with an optional retry-after duration.
    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited {
        /// Duration from the `Retry-After` header, if present.
        retry_after: Option<std::time::Duration>,
    },

    /// Local tokenizer error.
    #[error("tokenizer error: {0}")]
    Tokenizer(String),

    /// Benchmark sink write error.
    #[error("benchmark sink error: {0}")]
    Sink(String),

    /// Configuration error (invalid values, missing fields, etc.).
    #[error("config error: {0}")]
    Config(String),

    /// Evaluation runner error.
    #[error("eval error: {0}")]
    Eval(String),

    /// Wrapped I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Wrapped JSON parse/serialize error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Wrapped HTTP error (reqwest).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}
