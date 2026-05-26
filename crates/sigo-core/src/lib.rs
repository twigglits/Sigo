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
    load_corpus, load_default_corpus, read_jsonl, summarize, BenchmarkSink, CorpusEntry,
    CorpusLoadError, EnglishControlRun, JsonlSink, MemorySink, Summary, TurnRecord, SCHEMA_VERSION,
};
pub use claude::{ApiBackend, ClaudeBackend, ClaudeCodeBackend, FakeBackend, ResponseChunk, ScriptedItem};
pub use config::{BenchmarkConfig, ClaudeCodeConfig, ClaudeConfig, ReplConfig, SigoConfig, TranslatorConfig};
pub use conversation::{BackendKind, Conversation, Direction, Message, Role, Usage};
pub use error::{Result, SigoError};
pub use orchestrator::{CollectSink, ControlMode, Orchestrator, OrchestratorConfig, OutputSink, StdoutSink};
pub use stream::{Segment, SentenceBuffer};
pub use tokenizer::{ClaudeTokenizer, Tokenizer};
pub use translator::{FakeTranslator, OllamaTranslator, Translator};
