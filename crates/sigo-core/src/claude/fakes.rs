use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Usage};
use crate::error::Result;

/// Scripted backend for tests. Yields the supplied chunks in order.
pub struct FakeBackend {
    scripts: Arc<Mutex<Vec<Vec<(ResponseChunk, Duration)>>>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self { scripts: Arc::new(Mutex::new(vec![])) }
    }

    /// Queue the response for the next turn.
    pub fn enqueue_turn(&self, chunks: Vec<(ResponseChunk, Duration)>) {
        self.scripts.lock().unwrap().push(chunks);
    }

    /// Convenience: enqueue a single-text-then-done response.
    pub fn enqueue_simple(&self, text: &str, usage: Usage) {
        self.enqueue_turn(vec![
            (ResponseChunk::TextDelta(text.to_string()), Duration::from_millis(0)),
            (ResponseChunk::Done { usage, stop_reason: Some("end_turn".to_string()) }, Duration::from_millis(0)),
        ]);
    }
}

#[async_trait]
impl ClaudeBackend for FakeBackend {
    async fn stream_turn(
        &self,
        _convo: &Conversation,
        _prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        let next = self.scripts.lock().unwrap().drain(..1).next()
            .unwrap_or_else(|| vec![(ResponseChunk::Done { usage: Usage::default(), stop_reason: None }, Duration::from_millis(0))]);
        let s = stream::iter(next.into_iter().map(|(c, _d)| Ok(c)));
        Ok(Box::pin(s))
    }
}

impl Default for FakeBackend {
    fn default() -> Self { Self::new() }
}
