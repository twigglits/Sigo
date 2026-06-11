//! Benchmark logging, corpus management, and report generation.
//!
//! Every turn is recorded via [`BenchmarkSink`] (implemented by [`JsonlSink`]
//! for persistent storage and [`MemorySink`] for tests). The [`TurnRecord`]
//! captures local proxy token counts, Claude's reported usage, timing, and
//! an optional English control run.
//!
//! ## Corpus-driven runs
//!
//! [`load_corpus`] / [`load_default_corpus`] load prompt collections (JSONL or
//! plain text). [`build_markdown`] / [`build_csv`] produce per-run reports.
//! [`summarize`] aggregates across sessions.
//!
//! ## Coding benchmark
//!
//! [`load_coding_corpus`] / [`load_default_coding_corpus`] load HumanEval-format
//! tasks for objective code evaluation.

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

/// Persistent store for turn records.
///
/// Implementations must be [`Send`] + [`Sync`] and may be called concurrently
/// from multiple orchestrator instances.
pub trait BenchmarkSink: Send + Sync {
    /// Persist one turn record.
    fn record(&self, turn: &TurnRecord) -> Result<()>;
}

/// In-memory sink for tests. Stores records in a [`Mutex`]-guarded [`Vec`].
pub struct MemorySink {
    /// All recorded turns, in insertion order.
    pub records: std::sync::Mutex<Vec<TurnRecord>>,
}

impl MemorySink {
    /// Create an empty in-memory sink.
    pub fn new() -> Self {
        Self {
            records: std::sync::Mutex::new(vec![]),
        }
    }
    /// Snapshot of all recorded turns so far.
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
