//! EN↔ZH translation abstraction and Ollama implementation.
//!
//! The [`Translator`] trait is implemented by [`OllamaTranslator`] (production)
//! and [`FakeTranslator`] (tests). The module also provides code masking
//! ([`mask`]) and few-shot prompt templates ([`prompts`]).

use crate::conversation::Direction;
use crate::error::Result;

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
///
/// This trait uses native `async fn` (RPITIT, stable since Rust 1.75). It is NOT
/// dyn-compatible; use [`AnyTranslator`] for dynamic dispatch.
pub trait Translator: Send + Sync {
    /// Translate `text` in the given direction.
    async fn translate(&self, text: &str, dir: Direction) -> Result<String>;
}

/// Runtime-safe enum over all [`Translator`] implementations.
///
/// Prefer this over `Arc<dyn Translator>`: it enables native async dispatch
/// (no boxing, no vtable) and the set of backends is closed / checked at compile time.
#[derive(Debug, Clone)]
pub enum AnyTranslator {
    /// Ollama-based local translation (EN↔ZH).
    Ollama(OllamaTranslator),
    /// Test/bench stub with programmable responses.
    Fake(FakeTranslator),
}

impl Translator for AnyTranslator {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
        match self {
            Self::Ollama(inner) => inner.translate(text, dir).await,
            Self::Fake(inner) => inner.translate(text, dir).await,
        }
    }
}

pub use fakes::FakeTranslator;
#[doc(inline)]
pub use ollama::OllamaTranslator;
