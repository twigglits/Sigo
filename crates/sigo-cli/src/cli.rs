//! CLI argument parsing via clap. Defines the top-level [`Cli`] struct,
//! subcommands, and value-enum argument types.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Claude backend, validated at parse time. Maps to the `claude.backend` config string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    /// Anthropic Messages API.
    Api,
    /// Local Claude Code CLI.
    ClaudeCode,
}
impl BackendArg {
    /// Canonical config string for this backend.
    pub fn as_config_str(self) -> &'static str {
        match self {
            BackendArg::Api => "api",
            BackendArg::ClaudeCode => "claude-code",
        }
    }
}

/// Benchmark control mode, validated at parse time. Maps to `benchmark.control_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ControlModeArg {
    /// No control arm.
    Off,
    /// Local proxy token count only.
    PromptOnly,
    /// Full parallel English Claude call.
    Full,
}
impl ControlModeArg {
    /// Canonical config string for this control mode.
    pub fn as_config_str(self) -> &'static str {
        match self {
            ControlModeArg::Off => "off",
            ControlModeArg::PromptOnly => "prompt-only",
            ControlModeArg::Full => "full",
        }
    }
}

/// Output format for `bench export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    /// Newline-delimited JSON.
    Jsonl,
    /// Comma-separated values.
    Csv,
}

/// Evaluation mode for `bench run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EvalKind {
    /// Objective HumanEval-style coding benchmark.
    Coding,
}
impl EvalKind {
    /// Canonical string for this eval mode.
    pub fn as_str(self) -> &'static str {
        match self {
            EvalKind::Coding => "coding",
        }
    }
}

/// Sigo — Sino-Anglo translator + Claude benchmark CLI.
#[allow(missing_docs)]
#[derive(Debug, Parser)]
#[command(
    name = "sigo",
    version,
    about = "Sino-Anglo translator + Claude benchmark CLI"
)]
pub struct Cli {
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[arg(long)]
    pub backend: Option<BackendArg>,

    #[arg(long)]
    pub claude_model: Option<String>,

    #[arg(long)]
    pub translator_model: Option<String>,

    #[arg(long)]
    pub verbose: bool,

    #[arg(long)]
    pub control_mode: Option<ControlModeArg>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level subcommands.
#[allow(missing_docs)]
#[derive(Debug, Subcommand)]
pub enum Command {
    ConfigShow,
    Doctor,
    Bench {
        #[command(subcommand)]
        bench: BenchCommand,
    },
    Chat {
        prompt: Option<String>,
    },
}

/// Benchmark subcommands.
#[allow(missing_docs)]
#[derive(Debug, Subcommand)]
pub enum BenchCommand {
    Summary {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        last: Option<usize>,
    },
    Show {
        session: String,
        turn: u32,
    },
    Export {
        #[arg(long, default_value = "jsonl")]
        format: ExportFormat,
        #[arg(long)]
        session: Option<String>,
    },
    Run {
        #[arg(long)]
        corpus: Option<PathBuf>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        #[arg(long)]
        eval: Option<EvalKind>,
        #[arg(long, default_value_t = 1)]
        samples: usize,
        #[arg(long)]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn rejects_unknown_backend() {
        assert!(Cli::try_parse_from(["sigo", "--backend", "api2"]).is_err());
    }

    #[test]
    fn rejects_unknown_control_mode() {
        assert!(Cli::try_parse_from(["sigo", "--control-mode", "ful"]).is_err());
    }

    #[test]
    fn accepts_known_backend_and_control_mode() {
        let c = Cli::try_parse_from(["sigo", "--backend", "claude-code", "--control-mode", "full"])
            .unwrap();
        assert!(matches!(c.backend, Some(BackendArg::ClaudeCode)));
        assert!(matches!(c.control_mode, Some(ControlModeArg::Full)));
    }

    #[test]
    fn rejects_unknown_export_format_and_eval() {
        assert!(Cli::try_parse_from(["sigo", "bench", "export", "--format", "xml"]).is_err());
        assert!(Cli::try_parse_from(["sigo", "bench", "run", "--eval", "vibes"]).is_err());
    }
}
