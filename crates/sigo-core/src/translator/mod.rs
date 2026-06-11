//! EN↔ZH translation abstraction and Ollama implementation.
//!
//! The [`Translator`] trait is implemented by [`OllamaTranslator`] (production)
//! and [`FakeTranslator`] (tests). The module also provides code masking
//! ([`mask`]) and few-shot prompt templates ([`prompts`]).

use crate::conversation::Direction;
use crate::error::Result;
use async_trait::async_trait;

/// Test/bench stub translator with programmable responses.
pub mod fakes;
pub(crate) mod mask;
/// Ollama-based local translation (EN↔ZH).
pub mod ollama;
/// Few-shot prompt templates for the translation task.
pub mod prompts;
/// Input sanitization — strips control characters and injection markers
/// before sending user text to the local translator.
pub mod sanitize;

/// Bidirectional EN↔ZH translator.
#[async_trait]
pub trait Translator: Send + Sync {
    /// Translate `text` in the given direction.
    async fn translate(&self, text: &str, dir: Direction) -> Result<String>;
}

pub use fakes::FakeTranslator;
#[doc(inline)]
pub use ollama::OllamaTranslator;
