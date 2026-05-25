use crate::error::Result;

pub mod jsonl_sink;
pub mod turn_record;

pub use jsonl_sink::JsonlSink;
pub use turn_record::{EnglishControlRun, TurnRecord, SCHEMA_VERSION};

pub trait BenchmarkSink: Send + Sync {
    fn record(&self, turn: &TurnRecord) -> Result<()>;
}

/// In-memory sink for tests.
pub struct MemorySink {
    pub records: std::sync::Mutex<Vec<TurnRecord>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self { records: std::sync::Mutex::new(vec![]) }
    }
    pub fn snapshot(&self) -> Vec<TurnRecord> {
        self.records.lock().unwrap().clone()
    }
}

impl Default for MemorySink {
    fn default() -> Self { Self::new() }
}

impl BenchmarkSink for MemorySink {
    fn record(&self, turn: &TurnRecord) -> Result<()> {
        self.records.lock().unwrap().push(turn.clone());
        Ok(())
    }
}
