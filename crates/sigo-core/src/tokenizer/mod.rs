//! Token counting abstraction using a local `o200k_base` BPE proxy.
//!
//! [`TokenizerProxy`] wraps `tiktoken-rs`'s GPT-4o tokenizer as a
//! **directional proxy** for Claude's non-public tokenizer. Counts are
//! reported alongside Claude's authoritative numbers and must never be
//! presented as ground truth.
//!
//! # Accuracy caveat
//!
//! Claude's tokenizer is proprietary. The o200k_base BPE used here is a
//! rough proxy — particularly for CJK text, which tokenizes differently
//! between the two. All local counts are labelled "proxy" in reports.

use crate::error::Result;

pub mod proxy;

/// Count tokens in a text string.
///
/// All implementations are offline (no API calls) and must be [`Send`] + [`Sync`].
pub trait Tokenizer: Send + Sync {
    /// Return the token count for `text`.
    fn count_tokens(&self, text: &str) -> Result<u32>;
}

pub use proxy::TokenizerProxy;
