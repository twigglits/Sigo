pub mod code_exec;
pub mod eval_report;
pub mod fidelity;
pub mod metrics;

pub use code_exec::{evaluate_answer, extract_code, Outcome};
pub use eval_report::{build_eval_csv, build_eval_markdown, summarize_eval, ArmEval, EvalSummary, TaskEval};
pub use fidelity::{parse_score, roundtrip_fidelity, Judge, OllamaJudge};
pub use metrics::{ArmCost, bootstrap_ci_mean, pct_delta, win_rate, wilson_ci};
