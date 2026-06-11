use futures::stream::{self, BoxStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Usage};
use crate::error::{Result, SigoError};

/// One item in a scripted turn — either a happy-path chunk or an error to inject.
#[derive(Clone)]
pub enum ScriptedItem {
    /// Emit a normal chunk to the consumer.
    Chunk(ResponseChunk),
    /// Yield an error to the consumer, simulating a mid-stream failure.
    Error(String),
}

/// Queue of scripted turns; each turn is a list of timed scripted items.
type ScriptQueue = Arc<Mutex<Vec<Vec<(ScriptedItem, Duration)>>>>;

/// Fake [`ClaudeBackend`] for tests.
///
/// Scripted responses can be enqueued per-turn. Each call to `stream_turn` pops
/// the next script and replays its chunks/errors in order. The prompts passed to
/// each call are recorded for assertion.
pub struct FakeBackend {
    scripts: ScriptQueue,
    sent_prompts: Arc<Mutex<Vec<String>>>,
}

impl FakeBackend {
    /// Create a new fake backend with no scripts enqueued.
    pub fn new() -> Self {
        Self {
            scripts: Arc::new(Mutex::new(vec![])),
            sent_prompts: Arc::new(Mutex::new(vec![])),
        }
    }

    /// All prompts passed to `stream_turn`, in call order (including control-run calls).
    pub fn sent_prompts(&self) -> Vec<String> {
        self.sent_prompts.lock().unwrap().clone()
    }

    /// Queue an arbitrary scripted turn with per-chunk delays.
    ///
    /// Each call to `stream_turn` pops one turn from the queue. Each item in the
    /// turn is one [`ScriptedItem`] with a [`Duration`] delay before emission.
    pub fn enqueue_scripted(&self, items: Vec<(ScriptedItem, Duration)>) {
        self.scripts.lock().unwrap().push(items);
    }

    /// Queue a turn consisting of only [`ResponseChunk`] items (no injected errors).
    pub fn enqueue_turn(&self, chunks: Vec<(ResponseChunk, Duration)>) {
        let items = chunks
            .into_iter()
            .map(|(c, d)| (ScriptedItem::Chunk(c), d))
            .collect();
        self.enqueue_scripted(items);
    }

    /// Convenience: enqueue a single-text-then-done response with no delays.
    ///
    /// Creates a turn that emits one [`ResponseChunk::TextDelta`] followed by
    /// a [`ResponseChunk::Done`] with the given [`Usage`].
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

    /// Convenience: enqueue a turn that yields one text chunk then an error mid-stream.
    ///
    /// Useful for testing the orchestrator's error-recovery behaviour.
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

impl ClaudeBackend for FakeBackend {
    async fn stream_turn(
        &self,
        _convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        self.sent_prompts.lock().unwrap().push(prompt.to_string());
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
