use anyhow::{Context, Result};
use clap::Parser;
use sigo_cli::cli::{Cli, Command};
use sigo_cli::{commands, repl};
use sigo_core::SigoConfig;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Cli::parse();

    let mut config = if let Some(path) = &args.config {
        SigoConfig::load_from(path).context("loading config from --config path")?
    } else {
        SigoConfig::load().context("loading config")?
    };

    if let Some(b) = &args.backend {
        config.claude.backend = b.clone();
    }
    if let Some(m) = &args.claude_model {
        config.claude.model = m.clone();
    }
    if let Some(m) = &args.translator_model {
        config.translator.model = m.clone();
    }
    if let Some(c) = &args.control_mode {
        config.benchmark.control_mode = c.clone();
    }

    let verbose = args.verbose || config.repl.verbose;

    match args.command {
        None => repl::run(config, verbose).await,
        Some(Command::ConfigShow) => {
            println!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
        Some(Command::Doctor) => commands::doctor::run(&config).await,
        Some(Command::Bench { bench }) => commands::bench::run(&config, bench).await,
        Some(Command::Chat { prompt }) => commands::chat::run(&config, prompt, verbose).await,
    }
}
