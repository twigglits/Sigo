use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    pub backend: Option<String>,

    #[arg(long)]
    pub claude_model: Option<String>,

    #[arg(long)]
    pub translator_model: Option<String>,

    #[arg(long)]
    pub verbose: bool,

    #[arg(long)]
    pub control_mode: Option<String>,

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
        format: String,
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
        eval: Option<String>,
        /// Generations per task per arm (pass@k). Default 1.
        #[arg(long, default_value_t = 1)]
        samples: usize,
    },
}
