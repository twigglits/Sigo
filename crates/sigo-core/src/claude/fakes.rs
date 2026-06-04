use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Usage};
use crate::error::{Result, SigoError};

/// One item in a scripted turn — either a happy-path chunk or an error to inject.
#[derive(Clone)]
pub enum ScriptedItem {
    Chunk(ResponseChunk),
    Error(String),
}

/// Queue of scripted turns; each turn is a list of timed scripted items.
type ScriptQueue = Arc<Mutex<Vec<Vec<(ScriptedItem, Duration)>>>>;

pub struct FakeBackend {
    scripts: ScriptQueue,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self {
            scripts: Arc::new(Mutex::new(vec![])),
        }
    }

    /// Queue an arbitrary scripted turn (chunks + optional injected errors).
    pub fn enqueue_scripted(&self, items: Vec<(ScriptedItem, Duration)>) {
        self.scripts.lock().unwrap().push(items);
    }

    /// Backwards-compatible: queue a turn of just chunks (no errors).
    pub fn enqueue_turn(&self, chunks: Vec<(ResponseChunk, Duration)>) {
        let items = chunks
            .into_iter()
            .map(|(c, d)| (ScriptedItem::Chunk(c), d))
            .collect();
        self.enqueue_scripted(items);
    }

    /// Convenience: enqueue a single-text-then-done response.
    pub fn enqueue_simple(&self, text: &str, usage: Usage) {
        self.enqueue_turn(vec![
            (
                ResponseChunk::TextDelta(text.to_string()),
                Duration::from_millis(0),
            ),
            (
                ResponseChunk::Done {
                    usage,
                    stop_reason: Some("end_turn".to_string()),
                },
                Duration::from_millis(0),
            ),
        ]);
    }

    /// Convenience: enqueue a turn that yields one chunk then an error mid-stream.
    pub fn enqueue_error_after_chunk(&self, text: &str, error_msg: &str) {
        self.enqueue_scripted(vec![
            (
                ScriptedItem::Chunk(ResponseChunk::TextDelta(text.to_string())),
                Duration::from_millis(0),
            ),
            (
                ScriptedItem::Error(error_msg.to_string()),
                Duration::from_millis(0),
            ),
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
        let next = self
            .scripts
            .lock()
            .unwrap()
            .drain(..1)
            .next()
            .unwrap_or_else(|| {
                vec![(
                    ScriptedItem::Chunk(ResponseChunk::Done {
                        usage: Usage::default(),
                        stop_reason: None,
                    }),
                    Duration::from_millis(0),
                )]
            });
        let s = stream::iter(next.into_iter().map(|(item, _d)| match item {
            ScriptedItem::Chunk(c) => Ok(c),
            ScriptedItem::Error(msg) => Err(SigoError::Backend(msg)),
        }));
        Ok(Box::pin(s))
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}
