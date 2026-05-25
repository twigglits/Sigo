//! Sigo core library: translator/claude/tokenizer abstractions plus
//! per-turn orchestration for the Chinese-bridged Claude pipeline.

pub mod conversation;
pub mod error;
pub mod stream;
pub mod tokenizer;
pub mod translator;

pub use conversation::{BackendKind, Conversation, Direction, Message, Role, Usage};
pub use error::{Result, SigoError};
pub use stream::{Segment, SentenceBuffer};
pub use tokenizer::{ClaudeTokenizer, Tokenizer};
pub use translator::{OllamaTranslator, Translator};
