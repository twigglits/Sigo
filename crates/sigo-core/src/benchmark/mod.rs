use crate::error::Result;

pub mod coding_corpus;
pub mod corpus;
pub mod jsonl_sink;
pub mod run_report;
pub mod summary;
pub mod turn_record;

pub use coding_corpus::{load_coding_corpus, load_default_coding_corpus, CodingTask};
pub use corpus::{load_corpus, load_default_corpus, CorpusEntry, CorpusLoadError};
pub use jsonl_sink::JsonlSink;
pub use run_report::{
    build_csv, build_markdown, summarize_run, CategoryStats, RunReport, RunSummary,
};
pub use summary::{read_jsonl, summarize, Summary};
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
        Self {
            records: std::sync::Mutex::new(vec![]),
        }
    }
    pub fn snapshot(&self) -> Vec<TurnRecord> {
        self.records.lock().unwrap().clone()
    }
}

impl Default for MemorySink {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkSink for MemorySink {
    fn record(&self, turn: &TurnRecord) -> Result<()> {
        self.records.lock().unwrap().push(turn.clone());
        Ok(())
    }
}
