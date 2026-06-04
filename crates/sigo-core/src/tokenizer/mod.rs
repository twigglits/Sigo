use crate::error::Result;

pub mod proxy;

pub trait Tokenizer: Send + Sync {
    fn count_tokens(&self, text: &str) -> Result<u32>;
}

pub use proxy::TokenizerProxy;
