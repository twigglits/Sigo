//! Objective coding evaluation (HumanEval-style) and round-trip fidelity scoring.
//!
//! ## Coding benchmark
//!
//! [`evaluate_answer`] runs model-generated Python against hidden test cases
//! inside a sandboxed environment (bubblewrap when available, in-process
//! hardening always). [`extract_code`] pulls the solution out of a model's
//! markdown-formatted answer.
//!
//! ## Report generation
//!
//! [`summarize_eval`] computes paired EN-vs-ZH statistics across three layers
//! (proxy input tokens, reported input tokens, marginal dollar cost) with
//! bootstrap CIs and Wilson pass-rate confidence intervals.
//! [`build_eval_markdown`] / [`build_eval_csv`] produce human-readable and
//! analysis-friendly outputs.
//!
//! ## Round-trip fidelity
//!
//! [`roundtrip_fidelity`] uses a local Ollama judge to score whether every
//! fact, constraint, number, name, negation, and instruction survived the
//! EN→ZH→EN round trip. Diagnostic only — never a gate.

/// Code execution sandbox and answer evaluation.
pub mod code_exec;
/// Evaluation report generation (markdown, CSV, summaries).
pub mod eval_report;
/// Round-trip fidelity scoring via Ollama judge.
pub mod fidelity;
/// Statistical metrics (bootstrap CI, Wilson CI, win rates).
pub mod metrics;

pub use code_exec::{bwrap_works, evaluate_answer, extract_code, Outcome};
pub use eval_report::{
    build_eval_csv, build_eval_markdown, summarize_eval, ArmEval, EvalSummary, TaskEval,
};
pub use fidelity::{parse_score, roundtrip_fidelity, Judge, OllamaJudge};
pub use metrics::{bootstrap_ci_mean, pct_delta, wilson_ci, win_rate, ArmCost};
