use anyhow::{Context, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::sync::Arc;
use std::time::Duration;

use sigo_core::{
    ApiBackend, BackendKind, BenchmarkSink, ClaudeBackend, ClaudeCodeBackend, ClaudeTokenizer,
    ControlMode, JsonlSink, OllamaTranslator, Orchestrator, OrchestratorConfig, SigoConfig,
    StdoutSink, Tokenizer, Translator,
};

use crate::display::Display;

pub struct ReplState {
    pub orchestrator: Orchestrator,
    pub display: Display,
}

pub async fn run(config: SigoConfig, verbose: bool) -> Result<()> {
    let translator: Arc<dyn Translator> = Arc::new(OllamaTranslator::new(
        &config.translator.endpoint,
        &config.translator.model,
        Duration::from_secs(config.translator.timeout_seconds),
    ));

    let backend_kind = parse_backend(&config.claude.backend)?;
    let backend: Arc<dyn ClaudeBackend> = build_backend(backend_kind, &config)?;

    let tokenizer: Arc<dyn Tokenizer> = Arc::new(
        ClaudeTokenizer::new().context("failed to load bundled Claude tokenizer JSON")?,
    );

    let sink: Arc<dyn BenchmarkSink> = Arc::new(
        JsonlSink::open(config.resolved_log_path()).context("failed to open benchmark log")?,
    );

    let cfg = OrchestratorConfig {
        backend_kind,
        claude_model: config.claude.model.clone(),
        translator_model: config.translator.model.clone(),
        control_mode: ControlMode::parse(&config.benchmark.control_mode).unwrap_or(ControlMode::PromptOnly),
    };
    let orchestrator = Orchestrator::new(cfg, translator, backend, tokenizer, sink);

    let mut state = ReplState {
        orchestrator,
        display: Display::new(verbose),
    };

    let mut editor = DefaultEditor::new().context("rustyline init")?;
    let history_path = config.resolved_history_path();
    let _ = editor.load_history(&history_path);

    println!("sigo — type /help for commands, /quit to exit");
    loop {
        match editor.readline("» ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                editor.add_history_entry(line).ok();

                if let Some(rest) = line.strip_prefix('/') {
                    if !handle_slash(rest, &mut state, &config).await? {
                        break;
                    }
                } else {
                    let mut out = StdoutSink;
                    match state.orchestrator.run_turn(line, &mut out).await {
                        Ok(record) => state.display.print_turn_footer(&record),
                        Err(e) => eprintln!("turn failed: {e}"),
                    }
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }
    let _ = editor.save_history(&history_path);
    Ok(())
}

async fn handle_slash(rest: &str, state: &mut ReplState, config: &SigoConfig) -> Result<bool> {
    let mut parts = rest.split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    match cmd {
        "quit" | "exit" => return Ok(false),
        "help" => print_help(),
        "verbose" => {
            state.display.verbose = !state.display.verbose;
            println!("verbose: {}", state.display.verbose);
        }
        "reset" => {
            state.orchestrator.reset();
            println!("conversation reset (new session id = {})", state.orchestrator.session_id);
        }
        "control-mode" => {
            if let Some(arg) = args.first() {
                match ControlMode::parse(arg) {
                    Some(m) => {
                        state.orchestrator.config.control_mode = m;
                        println!("control-mode: {}", arg);
                    }
                    None => println!("invalid control-mode (off | prompt-only | full)"),
                }
            } else {
                println!("usage: /control-mode <off|prompt-only|full>");
            }
        }
        "model" => {
            if args.len() != 2 {
                println!("usage: /model <translator|claude> <name>");
            } else {
                match args[0] {
                    "translator" => {
                        let new_translator: Arc<dyn Translator> = Arc::new(OllamaTranslator::new(
                            &config.translator.endpoint,
                            args[1],
                            Duration::from_secs(config.translator.timeout_seconds),
                        ));
                        state.orchestrator.translator = new_translator;
                        state.orchestrator.config.translator_model = args[1].to_string();
                        println!("translator model: {}", args[1]);
                    }
                    "claude" => {
                        let bk = state.orchestrator.config.backend_kind;
                        let mut new_cfg = config.clone();
                        new_cfg.claude.model = args[1].to_string();
                        let new_backend = build_backend(bk, &new_cfg)?;
                        state.orchestrator.backend = new_backend;
                        state.orchestrator.config.claude_model = args[1].to_string();
                        println!(
                            "claude model: {} (note: prior cached tokens invalidated)",
                            args[1]
                        );
                    }
                    other => println!(
                        "unknown /model target `{other}` (expected `translator` or `claude`)"
                    ),
                }
            }
        }
        "backend" => {
            if let Some(arg) = args.first() {
                match parse_backend(arg) {
                    Ok(kind) => {
                        let new_backend = build_backend(kind, config)?;
                        state.orchestrator.backend = new_backend;
                        state.orchestrator.config.backend_kind = kind;
                        println!("backend: {arg} (note: prior cached tokens invalidated)");
                    }
                    Err(e) => println!("{e}"),
                }
            } else {
                println!("usage: /backend <api|claude-code>");
            }
        }
        "bench" => {
            let path = config.resolved_log_path();
            match sigo_core::read_jsonl(&path) {
                Ok(records) => {
                    let session_id = state.orchestrator.session_id;
                    let filtered: Vec<_> = records
                        .into_iter()
                        .filter(|r| r.session_id == session_id)
                        .collect();
                    let s = sigo_core::summarize(&filtered);
                    println!(
                        "[session {}] turns={}  zh-local-mean={:.1}  en-local-mean={:.1}",
                        session_id, s.turn_count, s.mean_zh_prompt_local, s.mean_en_prompt_local
                    );
                    if let Some(v) = s.estimated_savings_pct {
                        println!("            estimated savings: {:+.1}% vs EN", v);
                    }
                }
                Err(e) => println!("read bench log: {e}"),
            }
        }
        other => println!("unknown command: /{other} (try /help)"),
    }
    Ok(true)
}

fn print_help() {
    println!("commands:");
    println!("  /help                       show this list");
    println!("  /quit, /exit                leave the REPL");
    println!("  /verbose                    toggle verbose display");
    println!("  /reset                      start a new session");
    println!("  /control-mode <m>           off | prompt-only | full");
    println!("  /model translator <name>    swap translator model");
    println!("  /model claude <name>        swap Claude model");
    println!("  /backend <api|claude-code>  swap backend");
    println!("  /bench                      print current-session summary");
}

pub fn parse_backend(s: &str) -> Result<BackendKind> {
    match s {
        "api" => Ok(BackendKind::Api),
        "claude-code" => Ok(BackendKind::ClaudeCode),
        other => anyhow::bail!("unknown backend `{other}`; expected `api` or `claude-code`"),
    }
}

pub fn build_backend(kind: BackendKind, cfg: &SigoConfig) -> Result<Arc<dyn ClaudeBackend>> {
    match kind {
        BackendKind::Api => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY env var not set (required for `api` backend)")?;
            Ok(Arc::new(ApiBackend::new(key, &cfg.claude.model, cfg.claude.max_tokens)))
        }
        BackendKind::ClaudeCode => Ok(Arc::new(
            ClaudeCodeBackend::new(&cfg.claude.claude_code.binary)
                .with_extra_args(cfg.claude.claude_code.extra_args.clone())
                .with_model(cfg.claude.model.clone()),
        )),
    }
}
