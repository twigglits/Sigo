//! Subcommand implementations for the Sigo CLI.
//!
//! - [`chat`] — one-shot chat (single turn, answer to stdout)
//! - [`bench`] — benchmark analysis (summary, show, export)
//! - [`bench_run`] — corpus-driven benchmark runs with report generation
//! - [`checks`] — pre-flight connectivity checks
//! - [`doctor`] — full setup verification

/// Benchmark analysis subcommands (summary, show, export).
pub mod bench;
/// Corpus-driven benchmark runs with report generation.
pub mod bench_run;
pub mod chat;
pub mod checks;
pub mod doctor;
