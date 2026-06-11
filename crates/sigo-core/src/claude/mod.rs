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
/// AskUserQuestion passthrough types + stream-json control protocol.
pub mod question;

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
///
/// This trait uses native `async fn` (RPITIT, stable since Rust 1.75). It is NOT
/// dyn-compatible; use [`AnyClaudeBackend`] for dynamic dispatch.
pub trait ClaudeBackend: Send + Sync {
    /// Stream a turn. `convo` already contains prior history; `prompt` is the new user turn.
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>>;
}

/// Runtime-safe enum over all [`ClaudeBackend`] implementations.
///
/// Prefer this over `Arc<dyn ClaudeBackend>`: it enables native async dispatch
/// (no boxing, no vtable) and the set of backends is closed / checked at compile time.
#[derive(Debug, Clone)]
pub enum AnyClaudeBackend {
    /// Anthropic Messages API via HTTPS.
    Api(ApiBackend),
    /// Local `claude` CLI process (Claude Code).
    ClaudeCode(ClaudeCodeBackend),
    /// Test/bench stub with scripted responses.
    Fake(FakeBackend),
}

impl ClaudeBackend for AnyClaudeBackend {
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        match self {
            Self::Api(inner) => inner.stream_turn(convo, prompt).await,
            Self::ClaudeCode(inner) => inner.stream_turn(convo, prompt).await,
            Self::Fake(inner) => inner.stream_turn(convo, prompt).await,
        }
    }
}

pub use api::ApiBackend;
#[doc(inline)]
pub use claude_code::ClaudeCodeBackend;
pub use fakes::{FakeBackend, ScriptedItem};
pub use question::{AskOption, AskQuestion, QuestionAnswer, QuestionReply, QuestionRequest};
