//! Sigo core library: translator/claude/tokenizer abstractions plus
//! per-turn orchestration for the Chinese-bridged Claude pipeline.

pub mod conversation;
pub mod error;

pub use conversation::{BackendKind, Conversation, Direction, Message, Role, Usage};
pub use error::{Result, SigoError};
