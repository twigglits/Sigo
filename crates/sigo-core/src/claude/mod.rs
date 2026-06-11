//! Claude backend abstraction — stream a conversation turn and receive
//! [`ResponseChunk`] events.
//!
//! # Implementations
//!
//! | Backend | Description |
//! |---|---|
//! | [`ApiBackend`] | Anthropic Messages API via HTTPS |
//! | [`ClaudeCodeBackend`] | Local `claude` CLI process (Claude Code) |
//! | [`FakeBackend`] | Test/bench stub with scripted responses |

use futures::stream::BoxStream;

use crate::conversation::{Conversation, Usage};
use crate::error::Result;

/// Anthropic Messages API backend.
pub mod api;
/// Local `claude` CLI process backend (Claude Code).
pub mod claude_code;
/// Test/bench stub with scripted responses.
pub mod fakes;

/// One event from the Claude stream.
#[derive(Debug, Clone)]
pub enum ResponseChunk {
    /// A text delta fragment.
    TextDelta(String),
    /// The stream is complete with the given token usage and optional stop reason.
    Done {
        /// Token usage reported by Claude.
        usage: Usage,
        /// Stop reason (e.g. `"end_turn"`, `"max_tokens"`).
        stop_reason: Option<String>,
    },
}

/// Stream a conversation turn through a Claude backend.
///
/// Implementations handle the transport (HTTPS or sub-process), authentication,
/// and SSE/NDJSON parsing.
pub trait ClaudeBackend: Send + Sync {
    /// Stream a turn. `convo` already contains prior history; `prompt` is the new user turn.
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>>;
}

pub use api::ApiBackend;
#[doc(inline)]
pub use claude_code::ClaudeCodeBackend;
#[doc(inline)]
pub use fakes::{FakeBackend, ScriptedItem};
