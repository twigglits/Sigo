use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Claude backend, validated at parse time. Maps to the `claude.backend` config string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    Api,
    ClaudeCode,
}
impl BackendArg {
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
    Off,
    PromptOnly,
    Full,
}
impl ControlModeArg {
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
    Jsonl,
    Csv,
}

/// Evaluation mode for `bench run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EvalKind {
    Coding,
}
impl EvalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EvalKind::Coding => "coding",
        }
    }
}

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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the resolved configuration after all overrides.
    ConfigShow,
    /// Connectivity & setup checks.
    Doctor,
    /// Benchmark analysis subcommands.
    Bench {
        #[command(subcommand)]
        bench: BenchCommand,
    },
    /// Run a single turn non-interactively. Prompt from the argument, or stdin if omitted.
    /// Answer goes to stdout; a one-line summary goes to stderr with --verbose.
    Chat {
        /// The English prompt. If omitted, read the whole of stdin.
        prompt: Option<String>,
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
    /// Drive a corpus of prompts through the orchestrator and write a report.
    Run {
        #[arg(long)]
        corpus: Option<PathBuf>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Evaluation mode. Currently only `coding` (objective test execution).
        #[arg(long)]
        eval: Option<EvalKind>,
        /// Generations per task per arm (pass@k). Default 1.
        #[arg(long, default_value_t = 1)]
        samples: usize,
        /// Emit the run summary as JSON on stdout (progress stays on stderr).
        #[arg(long)]
        json: bool,
    },
}
