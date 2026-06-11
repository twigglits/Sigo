//! Sigo core library: translator/claude/tokenizer abstractions plus
//! per-turn orchestration for the Chinese-bridged Claude pipeline.
//!
//! # Architecture
//!
//! The pipeline flows:
//! 1. EN prompt → [`Translator::translate`] (local Ollama, EN→ZH)
//! 2. ZH prompt → [`Orchestrator`] sends to [`ClaudeBackend`] (Anthropic API or Claude Code CLI)
//! 3. Claude's ZH response → [`SentenceBuffer`] segments by sentence boundary
//! 4. Each ZH segment → [`Translator::translate`] (ZH→EN), emitted in order
//!
//! Token counts are tracked locally (via [`tokenizer::TokenizerProxy`] using o200k_base)
//! and authoritatively via Claude's reported usage. The [`benchmark`] module records every
//! turn to JSONL for analysis. The [`eval`] module drives objective coding benchmarks.
//!
//! # Key traits
//!
//! | Trait | Role | Implementations |
//! |---|---|---|
//! | [`Translator`](translator::Translator) | EN↔ZH translation | [`OllamaTranslator`], [`FakeTranslator`](translator::FakeTranslator) |
//! | [`ClaudeBackend`](claude::ClaudeBackend) | Claude API/CLI streaming | [`ApiBackend`], [`ClaudeCodeBackend`], [`FakeBackend`](claude::FakeBackend) |
//! | [`Tokenizer`](tokenizer::Tokenizer) | Token counting | [`TokenizerProxy`] |
//! | [`BenchmarkSink`](benchmark::BenchmarkSink) | Turn recording | [`JsonlSink`], [`MemorySink`](benchmark::MemorySink) |
//! | [`OutputSink`](orchestrator::OutputSink) | Streamed output | [`StdoutSink`](orchestrator::StdoutSink), [`CollectSink`](orchestrator::CollectSink) |
//!
//! # Feature flags
//!
//! - `live` — enable integration tests that hit a real Ollama or Anthropic API.

#![warn(missing_docs)]

pub mod benchmark;
pub mod claude;
pub mod compact;
pub mod config;
pub mod conversation;
pub mod error;
pub mod eval;
pub mod orchestrator;
pub mod stream;
pub mod tokenizer;
pub mod translator;

/// Benchmark module re-exports.
pub use benchmark::{
    build_csv, build_markdown, load_coding_corpus, load_corpus, load_default_coding_corpus,
    load_default_corpus, read_jsonl, summarize, summarize_run, BenchmarkSink, CategoryStats,
    CodingTask, CorpusEntry, CorpusLoadError, EnglishControlRun, JsonlSink, MemorySink, RunReport,
    RunSummary, Summary, TurnRecord, SCHEMA_VERSION,
};
/// Claude backend re-exports.
pub use claude::{
    AnyClaudeBackend, ApiBackend, ClaudeBackend, ClaudeCodeBackend, FakeBackend, ResponseChunk,
    ScriptedItem,
};
/// Chinese text compaction.
pub use compact::compact_zh;
/// Configuration re-exports.
pub use config::{
    apply_env_overlay, BenchmarkConfig, ClaudeCodeConfig, ClaudeConfig, PricingConfig, ReplConfig,
    SigoConfig, TranslatorConfig, TranslatorStyle,
};
/// Conversation type re-exports.
pub use conversation::{BackendKind, Conversation, Direction, Message, Role, Usage};
/// Error type re-exports.
pub use error::{Result, SigoError};
/// Eval module re-exports.
pub use eval::{
    build_eval_csv, build_eval_markdown, bwrap_works, evaluate_answer, extract_code,
    roundtrip_fidelity, summarize_eval, ArmCost, ArmEval, EvalSummary, Judge, OllamaJudge, Outcome,
    TaskEval,
};
/// Orchestrator re-exports.
pub use orchestrator::{
    CollectSink, ControlMode, Orchestrator, OrchestratorConfig, OutputSink, StdoutSink,
};
/// Stream types re-exports.
pub use stream::{Segment, SentenceBuffer};
/// Tokenizer re-exports.
pub use tokenizer::{Tokenizer, TokenizerProxy};
/// Translator re-exports.
pub use translator::{sanitize, AnyTranslator, FakeTranslator, OllamaTranslator, Translator};
