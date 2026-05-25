use thiserror::Error;

pub type Result<T, E = SigoError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum SigoError {
    #[error("translator error: {0}")]
    Translator(String),

    #[error("translator timed out after {0:?}")]
    TranslatorTimeout(std::time::Duration),

    #[error("claude backend error: {0}")]
    Backend(String),

    #[error("claude stream disconnected mid-turn after {bytes_received} bytes")]
    StreamDisconnect { bytes_received: usize },

    #[error("claude auth failed: {0}")]
    Auth(String),

    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited { retry_after: Option<std::time::Duration> },

    #[error("tokenizer error: {0}")]
    Tokenizer(String),

    #[error("benchmark sink error: {0}")]
    Sink(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}
