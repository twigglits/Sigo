use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::Write;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc;

use sigo_core::{
    AnyClaudeBackend, AnyTranslator, ApiBackend, BackendKind, BenchmarkSink, ClaudeCodeBackend,
    ControlMode, JsonlSink, OllamaTranslator, Orchestrator, OrchestratorConfig, QuestionRequest,
    SigoConfig, Tokenizer, TokenizerProxy,
};

use crate::display::Display;
use crate::question_bridge::{SharedTranslator, SpinnerSlot};

/// State preserved across REPL turns.
pub struct ReplState {
    /// The orchestrator managing conversation and translation.
    pub orchestrator: Orchestrator,
    /// Display configuration (verbose mode, etc.).
    pub display: Display,
    /// Sender for AskUserQuestion passthrough; re-attached to the backend
    /// after `/backend` and `/model claude` swaps. `None` when interactive
    /// mode is disabled in config.
    pub question_tx: Option<mpsc::Sender<QuestionRequest>>,
    /// Translator shared with the question bridge (kept in sync by
    /// `/model translator`).
    pub bridge_translator: SharedTranslator,
    /// The active turn's spinner, suspended while the picker is on screen.
    pub spinner_slot: SpinnerSlot,
}

/// Re-attach the question channel after the backend is built or swapped.
fn attach_question_channel(state: &mut ReplState) {
    if let (AnyClaudeBackend::ClaudeCode(b), Some(tx)) =
        (&mut state.orchestrator.backend, &state.question_tx)
    {
        b.attach_question_channel(tx.clone());
    }
}

/// Interactive claude-code turns are one-at-a-time, so the parallel English
/// control run of `control_mode=full` cannot work there. Warn instead of
/// failing mysteriously mid-turn.
fn warn_if_full_control_with_interactive(state: &ReplState) {
    if state.orchestrator.config.control_mode == ControlMode::Full
        && state.question_tx.is_some()
        && state.orchestrator.config.backend_kind == BackendKind::ClaudeCode
    {
        println!(
            "warning: control-mode=full runs a parallel English turn, which the interactive \
             claude-code backend rejects (one turn at a time). Use the api backend or set \
             claude_code.interactive = false."
        );
    }
}

/// Reset the backend's own conversation state (the claude-code CLI session),
/// so `/reset` and `/clear` truly start over instead of silently resuming.
async fn reset_backend_session(backend: &AnyClaudeBackend) {
    if let AnyClaudeBackend::ClaudeCode(b) = backend {
        b.reset_session().await;
    }
}

/// Build the full orchestrator stack (translator + backend + tokenizer + sink) from config.
/// Shared by the REPL and the one-shot `chat` command.
pub fn build_orchestrator(config: &SigoConfig) -> Result<Orchestrator> {
    let translator = AnyTranslator::Ollama(
        OllamaTranslator::new(
            &config.translator.endpoint,
            &config.translator.model,
            Duration::from_secs(config.translator.timeout_seconds),
        )
        .with_style(config.translator.style),
    );

    let backend_kind = parse_backend(&config.claude.backend)?;
    let backend = build_backend(backend_kind, config)?;

    let tokenizer: Arc<dyn Tokenizer> =
        Arc::new(TokenizerProxy::new().context("failed to initialize o200k_base proxy tokenizer")?);

    let sink: Arc<dyn BenchmarkSink> = Arc::new(
        JsonlSink::open(config.resolved_log_path()).context("failed to open benchmark log")?,
    );

    let cfg = OrchestratorConfig {
        backend_kind,
        claude_model: config.claude.model.clone(),
        translator_model: config.translator.model.clone(),
        control_mode: ControlMode::parse(&config.benchmark.control_mode).with_context(|| {
            format!(
                "invalid benchmark.control_mode `{}` (expected off | prompt-only | full)",
                config.benchmark.control_mode
            )
        })?,
    };
    Ok(Orchestrator::new(cfg, translator, backend, tokenizer, sink))
}

/// Run the interactive REPL.
pub async fn run(config: SigoConfig, verbose: bool) -> Result<()> {
    crate::commands::checks::preflight_translator(&config)
        .await
        .context("translator preflight failed")?;

    let orchestrator = build_orchestrator(&config)?;

    let bridge_translator: SharedTranslator =
        Arc::new(StdMutex::new(orchestrator.translator.clone()));
    let spinner_slot: SpinnerSlot = Arc::default();
    let question_tx = if config.claude.claude_code.interactive {
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(crate::question_bridge::run_bridge(
            rx,
            bridge_translator.clone(),
            spinner_slot.clone(),
        ));
        Some(tx)
    } else {
        None
    };

    let mut state = ReplState {
        orchestrator,
        display: Display::new(verbose),
        question_tx,
        bridge_translator,
        spinner_slot,
    };
    attach_question_channel(&mut state);
    warn_if_full_control_with_interactive(&state);

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
                    let mut out = SpinnerSink::with_slot(state.spinner_slot.clone());
                    match state.orchestrator.run_turn(line, &mut out).await {
                        Ok(record) => state.display.print_turn_footer(&record),
                        Err(e) => eprintln!("turn failed: {e}"),
                    }
                    *state.spinner_slot.lock().unwrap() = None;
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

/// A sink that displays a spinner until the first chunk of text is written.
pub struct SpinnerSink {
    pb: ProgressBar,
    first_chunk: bool,
}

impl Default for SpinnerSink {
    fn default() -> Self {
        Self::new()
    }
}

impl SpinnerSink {
    /// Build a new `SpinnerSink` with the given message prefix.
    pub fn new() -> Self {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.green} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠲⠁"),
        );
        pb.set_message("Sigo is thinking...");
        pb.enable_steady_tick(Duration::from_millis(100));
        Self {
            pb,
            first_chunk: true,
        }
    }

    /// Build a `SpinnerSink` and register its progress bar in `slot` so the
    /// question bridge can suspend it while a picker is on screen.
    pub fn with_slot(slot: SpinnerSlot) -> Self {
        let sink = Self::new();
        *slot.lock().unwrap() = Some(sink.pb.clone());
        sink
    }
}

impl sigo_core::OutputSink for SpinnerSink {
    fn write(&mut self, s: &str) {
        if self.first_chunk {
            self.pb.finish_and_clear();
            self.first_chunk = false;
        }
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }
    fn flush(&mut self) {}
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
            reset_backend_session(&state.orchestrator.backend).await;
            println!(
                "conversation reset (new session id = {})",
                state.orchestrator.session_id
            );
        }
        "clear" => {
            state.orchestrator.clear();
            reset_backend_session(&state.orchestrator.backend).await;
            println!(
                "conversation cleared (session {} continues)",
                state.orchestrator.session_id
            );
        }
        "save" => {
            if let Some(arg) = args.first() {
                let path = std::path::PathBuf::from(arg);
                match save_session(&state.orchestrator, &path) {
                    Ok(()) => println!("session saved to {}", path.display()),
                    Err(e) => println!("save failed: {e}"),
                }
            } else {
                println!("usage: /save <path>");
            }
        }
        "control-mode" => {
            if let Some(arg) = args.first() {
                match ControlMode::parse(arg) {
                    Some(m) => {
                        state.orchestrator.config.control_mode = m;
                        println!("control-mode: {}", arg);
                        warn_if_full_control_with_interactive(state);
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
                        let new_translator = AnyTranslator::Ollama(
                            OllamaTranslator::new(
                                &config.translator.endpoint,
                                args[1],
                                Duration::from_secs(config.translator.timeout_seconds),
                            )
                            .with_style(config.translator.style),
                        );
                        *state.bridge_translator.lock().unwrap() = new_translator.clone();
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
                        attach_question_channel(state);
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
                        attach_question_channel(state);
                        warn_if_full_control_with_interactive(state);
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
    println!("  /reset                      start a new session with a new ID");
    println!("  /clear                      clear history but keep the same session");
    println!("  /save <path>                dump the current session to a markdown file");
    println!("  /control-mode <m>           off | prompt-only | full");
    println!("  /model translator <name>    swap translator model");
    println!("  /model claude <name>        swap Claude model");
    println!("  /backend <api|claude-code>  swap backend");
    println!("  /bench                      print current-session summary");
}

/// Parse a backend string (`"api"` or `"claude-code"`) into a [`BackendKind`].
pub fn parse_backend(s: &str) -> Result<BackendKind> {
    match s {
        "api" => Ok(BackendKind::Api),
        "claude-code" => Ok(BackendKind::ClaudeCode),
        other => anyhow::bail!("unknown backend `{other}`; expected `api` or `claude-code`"),
    }
}

/// Build a Claude backend from its kind and config.
pub fn build_backend(kind: BackendKind, cfg: &SigoConfig) -> Result<AnyClaudeBackend> {
    match kind {
        BackendKind::Api => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY env var not set (required for `api` backend)")?;
            Ok(AnyClaudeBackend::Api(ApiBackend::with_options(
                key,
                &cfg.claude.model,
                cfg.claude.max_tokens,
                cfg.claude.temperature,
                cfg.claude.top_p,
            )))
        }
        BackendKind::ClaudeCode => Ok(AnyClaudeBackend::ClaudeCode(
            ClaudeCodeBackend::new(&cfg.claude.claude_code.binary)
                .with_extra_args(cfg.claude.claude_code.extra_args.clone())
                .with_model(cfg.claude.model.clone()),
        )),
    }
}

fn save_session(orch: &Orchestrator, path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    writeln!(
        file,
        "# Sigo session {} — {} turns",
        orch.session_id, orch.turn_index
    )?;
    writeln!(file)?;
    for (i, (zh, en)) in orch
        .chinese_convo
        .messages
        .iter()
        .zip(orch.english_convo.messages.iter())
        .enumerate()
    {
        writeln!(file, "## Turn {} — {:?}", i / 2 + 1, zh.role)?;
        writeln!(file, "### English")?;
        writeln!(file, "{}", en.content)?;
        writeln!(file)?;
        writeln!(file, "### Chinese")?;
        writeln!(file, "{}", zh.content)?;
        writeln!(file)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sigo_core::{FakeBackend, FakeTranslator, MemorySink};

    fn fake_state(question_tx: Option<mpsc::Sender<QuestionRequest>>) -> ReplState {
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::ClaudeCode,
            claude_model: "m".into(),
            translator_model: "t".into(),
            control_mode: ControlMode::PromptOnly,
        };
        let translator = AnyTranslator::Fake(FakeTranslator::new());
        let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
        let orchestrator = Orchestrator::new(
            cfg,
            translator.clone(),
            AnyClaudeBackend::Fake(FakeBackend::new()),
            tokenizer,
            Arc::new(MemorySink::new()),
        );
        ReplState {
            orchestrator,
            display: Display::new(false),
            question_tx,
            bridge_translator: Arc::new(StdMutex::new(translator)),
            spinner_slot: Arc::default(),
        }
    }

    #[tokio::test]
    async fn backend_swap_to_claude_code_reattaches_question_channel() {
        let (tx, _rx) = mpsc::channel(1);
        let mut state = fake_state(Some(tx));
        let config = SigoConfig::default();

        handle_slash("backend claude-code", &mut state, &config)
            .await
            .unwrap();

        match &state.orchestrator.backend {
            AnyClaudeBackend::ClaudeCode(b) => assert!(
                b.has_question_channel(),
                "swapped-in backend must keep the interactive picker working"
            ),
            other => panic!("expected claude-code backend, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn backend_swap_without_channel_stays_non_interactive() {
        let mut state = fake_state(None);
        let config = SigoConfig::default();

        handle_slash("backend claude-code", &mut state, &config)
            .await
            .unwrap();

        match &state.orchestrator.backend {
            AnyClaudeBackend::ClaudeCode(b) => assert!(!b.has_question_channel()),
            other => panic!("expected claude-code backend, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn claude_model_swap_preserves_question_channel() {
        let (tx, _rx) = mpsc::channel(1);
        let mut state = fake_state(Some(tx));
        let config = SigoConfig::default();
        handle_slash("backend claude-code", &mut state, &config)
            .await
            .unwrap();

        handle_slash("model claude claude-sonnet-4-6", &mut state, &config)
            .await
            .unwrap();

        match &state.orchestrator.backend {
            AnyClaudeBackend::ClaudeCode(b) => assert!(
                b.has_question_channel(),
                "/model claude must not silently drop the picker"
            ),
            other => panic!("expected claude-code backend, got {other:?}"),
        }
        assert_eq!(state.orchestrator.config.claude_model, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn reset_and_clear_run_against_any_backend_without_error() {
        let mut state = fake_state(None);
        let config = SigoConfig::default();
        // Fake backend: reset_backend_session is a no-op but must not panic.
        assert!(handle_slash("reset", &mut state, &config).await.unwrap());
        assert!(handle_slash("clear", &mut state, &config).await.unwrap());
    }
}
