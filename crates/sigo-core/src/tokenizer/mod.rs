use crate::error::Result;

pub mod claude;

pub trait Tokenizer: Send + Sync {
    fn count_tokens(&self, text: &str) -> Result<u32>;
}

pub use claude::ClaudeTokenizer;
