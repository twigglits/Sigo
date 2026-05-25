use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::conversation::{Conversation, Usage};
use crate::error::Result;

pub mod api;
pub mod claude_code;
pub mod fakes;

#[derive(Debug, Clone)]
pub enum ResponseChunk {
    TextDelta(String),
    Done { usage: Usage, stop_reason: Option<String> },
}

#[async_trait]
pub trait ClaudeBackend: Send + Sync {
    /// Stream a turn. `convo` already contains prior history; `prompt` is the new user turn.
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>>;
}

pub use api::ApiBackend;
pub use claude_code::ClaudeCodeBackend;
pub use fakes::FakeBackend;
