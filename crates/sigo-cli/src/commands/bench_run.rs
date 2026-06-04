use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sigo_core::{
    build_csv, build_markdown, load_corpus, summarize_run, BackendKind, BenchmarkSink,
    ClaudeBackend, TokenizerProxy, ControlMode, CorpusEntry, JsonlSink, OllamaTranslator,
    Orchestrator, OrchestratorConfig, OutputSink, RunReport, SigoConfig, Tokenizer, Translator,
    TurnRecord,
};

use crate::repl::build_backend;

pub struct RunOptions {
    pub corpus_path: Option<PathBuf>,
    pub label: Option<String>,
    pub limit: Option<usize>,
    pub out_dir: Option<PathBuf>,
}

pub type TranslatorBuilder = Arc<dyn Fn() -> Arc<dyn Translator> + Send + Sync>;
pub type BackendBuilder = Arc<dyn Fn() -> Result<Arc<dyn ClaudeBackend>> + Send + Sync>;

pub async fn run(cfg: &SigoConfig, opts: RunOptions) -> Result<()> {
    // Validate backend early so a typo doesn't slip through builders.
    let backend_kind = parse_backend_kind(&cfg.claude.backend)?;
    let cfg_for_tx = cfg.clone();
    let translator_builder: TranslatorBuilder = Arc::new(move || {
        Arc::new(OllamaTranslator::new(
            &cfg_for_tx.translator.endpoint,
            &cfg_for_tx.translator.model,
            Duration::from_secs(cfg_for_tx.translator.timeout_seconds),
        )) as Arc<dyn Translator>
    });
    let cfg_for_be = cfg.clone();
    let backend_builder: BackendBuilder = Arc::new(move || {
        build_backend(backend_kind, &cfg_for_be)
    });
    run_with_builders(cfg, opts, translator_builder, backend_builder).await
}

pub async fn run_with_builders(
    cfg: &SigoConfig,
    opts: RunOptions,
    translator_builder: TranslatorBuilder,
    backend_builder: BackendBuilder,
) -> Result<()> {
    let backend_kind = parse_backend_kind(&cfg.claude.backend)?;

    // Pre-flight: catch backend misconfiguration (e.g. missing ANTHROPIC_API_KEY) before the loop
    // does setup work that the user would then have to undo.
    backend_builder()
        .context("pre-flight: failed to construct backend (check config and env vars)")?;

    let corpus = load_corpus(opts.corpus_path.as_deref())
        .map_err(|e| anyhow::anyhow!("corpus load: {e}"))?;
    if let Some(0) = opts.limit {
        anyhow::bail!("--limit must be at least 1");
    }
    let corpus: Vec<CorpusEntry> = match opts.limit {
        Some(n) => corpus.into_iter().take(n).collect(),
        None => corpus,
    };
    if corpus.is_empty() {
        anyhow::bail!("corpus is empty after applying --limit");
    }

    let run_id = build_run_id(opts.label.as_deref());
    let out_dir = opts.out_dir.unwrap_or_else(|| default_run_dir(&run_id));
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create out_dir {}", out_dir.display()))?;

    let tokenizer: Arc<dyn Tokenizer> = Arc::new(
        TokenizerProxy::new().context("failed to initialize o200k_base proxy tokenizer")?,
    );
    let sink: Arc<dyn BenchmarkSink> = Arc::new(
        JsonlSink::open(cfg.resolved_log_path()).context("failed to open benchmark log")?,
    );

    let errors_path = out_dir.join("errors.jsonl");
    let mut errors_handle: Option<std::fs::File> = None;
    let corpus_source = opts
        .corpus_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "bundled".into());

    let started_at = Utc::now();
    let total = corpus.len();
    let mut rows: Vec<(TurnRecord, String)> = Vec::with_capacity(total);
    let mut n_failed = 0usize;

    println!(
        "sigo bench run · run_id={} · backend={} · {} prompts",
        run_id, cfg.claude.backend, total
    );

    for (i, entry) in corpus.iter().enumerate() {
        // Fresh translator + fresh backend per prompt so each prompt is turn-0 of its own session.
        let translator = translator_builder();
        let backend = backend_builder()
            .with_context(|| format!("build backend for prompt {} ({})", i + 1, entry.category))?;
        let mut orch = Orchestrator::new(
            OrchestratorConfig {
                backend_kind,
                claude_model: cfg.claude.model.clone(),
                translator_model: cfg.translator.model.clone(),
                control_mode: ControlMode::Full,
            },
            translator,
            backend,
            tokenizer.clone(),
            sink.clone(),
        );

        let mut out = SilentSink;
        let start = std::time::Instant::now();
        match orch.run_turn(&entry.prompt, &mut out).await {
            Ok(record) => {
                print_progress(i + 1, total, entry, &record, start.elapsed());
                rows.push((record, entry.category.clone()));
            }
            Err(e) => {
                n_failed += 1;
                if errors_handle.is_none() {
                    errors_handle = Some(
                        std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&errors_path)
                            .with_context(|| format!("open {}", errors_path.display()))?,
                    );
                }
                let handle = errors_handle.as_mut().expect("errors_handle just initialised");
                write_error_line(handle, i + 1, entry, "claude_open_or_translator", &e.to_string())?;
                eprintln!(
                    "[{}/{}] {} · FAILED: {}",
                    i + 1,
                    total,
                    entry.category,
                    e
                );
            }
        }
    }
    let finished_at = Utc::now();
    drop(errors_handle);

    let summary = summarize_run(
        run_id.clone(),
        started_at,
        finished_at,
        backend_kind,
        cfg.claude.model.clone(),
        cfg.translator.model.clone(),
        corpus_source,
        total,
        n_failed,
        &rows,
    );
    let report = RunReport {
        summary: summary.clone(),
        rows,
    };
    let md_path = out_dir.join("report.md");
    let csv_path = out_dir.join("report.csv");
    std::fs::write(&md_path, build_markdown(&report))
        .with_context(|| format!("write {}", md_path.display()))?;
    std::fs::write(&csv_path, build_csv(&report))
        .with_context(|| format!("write {}", csv_path.display()))?;

    let dpct = if summary.mean_en_input_reported > 0.0 {
        Some(
            (summary.mean_zh_input_reported - summary.mean_en_input_reported)
                / summary.mean_en_input_reported
                * 100.0,
        )
    } else {
        None
    };
    let dstr = dpct
        .map(|v| format!("{:+.1}%", v))
        .unwrap_or_else(|| "n/a".into());
    println!(
        "bench run complete: {}/{} succeeded · ZH input {} vs EN · report: {}",
        summary.n_succeeded,
        total,
        dstr,
        md_path.display()
    );
    Ok(())
}

fn parse_backend_kind(s: &str) -> Result<BackendKind> {
    match s {
        "api" => Ok(BackendKind::Api),
        "claude-code" => Ok(BackendKind::ClaudeCode),
        other => anyhow::bail!("unknown backend `{other}` (expected `api` or `claude-code`)"),
    }
}

fn build_run_id(label: Option<&str>) -> String {
    let ts = Utc::now().format("%Y-%m-%dT%H%M%S").to_string();
    match label {
        Some(l) => format!("{ts}-{}", slug(l)),
        None => ts,
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn default_run_dir(run_id: &str) -> PathBuf {
    let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    data.join("sigo").join("runs").join(run_id)
}

fn print_progress(
    idx: usize,
    total: usize,
    entry: &CorpusEntry,
    r: &TurnRecord,
    elapsed: std::time::Duration,
) {
    let zh_in = r.chinese_prompt_tokens_reported.unwrap_or(0);
    let en_in = r
        .english_control_run
        .as_ref()
        .map(|c| c.prompt_tokens_reported)
        .unwrap_or(0);
    let dpct = if en_in > 0 {
        format!(
            "{:+.0}%",
            (zh_in as f64 - en_in as f64) / en_in as f64 * 100.0
        )
    } else {
        "n/a".into()
    };
    let inc_marker = if r.incomplete { " · incomplete" } else { "" };
    println!(
        "[{idx}/{total}] {cat} · zh-in={zh_in} en-in={en_in} ({dpct}) · {:.1}s{inc}",
        elapsed.as_secs_f64(),
        cat = entry.category,
        inc = inc_marker,
    );
}

#[derive(Serialize)]
struct ErrorLine<'a> {
    timestamp: String,
    prompt_index: usize,
    category: &'a str,
    prompt: &'a str,
    stage: &'a str,
    error: &'a str,
}

fn write_error_line(
    handle: &mut std::fs::File,
    idx: usize,
    entry: &CorpusEntry,
    stage: &str,
    err: &str,
) -> Result<()> {
    use std::io::Write;
    let line = ErrorLine {
        timestamp: Utc::now().to_rfc3339(),
        prompt_index: idx,
        category: &entry.category,
        prompt: &entry.prompt,
        stage,
        error: err,
    };
    writeln!(handle, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

struct SilentSink;
impl OutputSink for SilentSink {
    fn write(&mut self, _: &str) {}
}
