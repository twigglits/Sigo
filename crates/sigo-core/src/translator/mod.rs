use crate::conversation::Direction;
use crate::error::Result;
use async_trait::async_trait;

pub mod fakes;
pub mod ollama;
pub mod prompts;

#[async_trait]
pub trait Translator: Send + Sync {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String>;
}

pub use fakes::FakeTranslator;
pub use ollama::OllamaTranslator;
