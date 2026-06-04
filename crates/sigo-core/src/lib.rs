//! Sigo core library: translator/claude/tokenizer abstractions plus
//! per-turn orchestration for the Chinese-bridged Claude pipeline.

pub mod benchmark;
pub mod claude;
pub mod config;
pub mod conversation;
pub mod error;
pub mod orchestrator;
pub mod stream;
pub mod tokenizer;
pub mod translator;

pub use benchmark::{
    build_csv, build_markdown, load_corpus, load_default_corpus, read_jsonl, summarize,
    summarize_run, BenchmarkSink, CategoryStats, CorpusEntry, CorpusLoadError, EnglishControlRun,
    JsonlSink, MemorySink, RunReport, RunSummary, Summary, TurnRecord, SCHEMA_VERSION,
};
pub use claude::{ApiBackend, ClaudeBackend, ClaudeCodeBackend, FakeBackend, ResponseChunk, ScriptedItem};
pub use config::{BenchmarkConfig, ClaudeCodeConfig, ClaudeConfig, PricingConfig, ReplConfig, SigoConfig, TranslatorConfig};
pub use conversation::{BackendKind, Conversation, Direction, Message, Role, Usage};
pub use error::{Result, SigoError};
pub use orchestrator::{CollectSink, ControlMode, Orchestrator, OrchestratorConfig, OutputSink, StdoutSink};
pub use stream::{Segment, SentenceBuffer};
pub use tokenizer::{TokenizerProxy, Tokenizer};
pub use translator::{FakeTranslator, OllamaTranslator, Translator};
