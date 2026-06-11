use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sigo_core::{
    build_csv, build_eval_csv, build_eval_markdown, build_markdown, evaluate_answer,
    load_coding_corpus, load_corpus, roundtrip_fidelity, summarize_eval, summarize_run,
    AnyClaudeBackend, AnyTranslator, ArmCost, ArmEval, BackendKind, BenchmarkSink, ControlMode,
    CorpusEntry, JsonlSink, OllamaJudge, OllamaTranslator, Orchestrator, OrchestratorConfig,
    OutputSink, RunReport, SigoConfig, TaskEval, Tokenizer, TokenizerProxy, TurnRecord,
};

use crate::repl::build_backend;

/// Options for a benchmark run.
#[allow(missing_docs)]
pub struct RunOptions {
    pub corpus_path: Option<PathBuf>,
    pub label: Option<String>,
    pub limit: Option<usize>,
    pub out_dir: Option<PathBuf>,
    pub eval: Option<String>,
    pub samples: usize,
    /// Emit the machine-readable run summary as a single JSON line on stdout. Human
    /// progress/status always goes to stderr, so `--json` output is clean for piping.
    pub json: bool,
}

/// Builder for translator instances (lazy, so the test suite can inject fakes).
pub type TranslatorBuilder = Arc<dyn Fn() -> AnyTranslator + Send + Sync>;
/// Builder for backend instances (lazy, so the test suite can inject fakes).
pub type BackendBuilder = Arc<dyn Fn() -> Result<AnyClaudeBackend> + Send + Sync>;

/// Run a benchmark from config and options.
pub async fn run(cfg: &SigoConfig, opts: RunOptions) -> Result<()> {
    // Validate backend early so a typo doesn't slip through builders.
    let backend_kind = parse_backend_kind(&cfg.claude.backend)?;
    let cfg_for_tx = cfg.clone();
    let translator_builder: TranslatorBuilder = Arc::new(move || {
        AnyTranslator::Ollama(
            OllamaTranslator::new(
                &cfg_for_tx.translator.endpoint,
                &cfg_for_tx.translator.model,
                Duration::from_secs(cfg_for_tx.translator.timeout_seconds),
            )
            .with_style(cfg_for_tx.translator.style),
        )
    });
    let cfg_for_be = cfg.clone();
    let backend_builder: BackendBuilder =
        Arc::new(move || build_backend(backend_kind, &cfg_for_be));
    run_with_builders(cfg, opts, translator_builder, backend_builder).await
}

/// Drive a corpus of prompts through the orchestrator and write a report.
///
/// Accepts builder closures for translator and backend so the test suite can
/// inject fake implementations.
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

    if opts.eval.as_deref() == Some("coding") {
        return run_coding_eval(
            cfg,
            &opts,
            backend_kind,
            &translator_builder,
            &backend_builder,
        )
        .await;
    }
    if let Some(other) = opts.eval.as_deref() {
        anyhow::bail!("unknown --eval mode `{other}` (only `coding` is supported)");
    }

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

    let tokenizer: Arc<dyn Tokenizer> =
        Arc::new(TokenizerProxy::new().context("failed to initialize o200k_base proxy tokenizer")?);
    let sink: Arc<dyn BenchmarkSink> =
        Arc::new(JsonlSink::open(cfg.resolved_log_path()).context("failed to open benchmark log")?);

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

    eprintln!(
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
                let handle = errors_handle
                    .as_mut()
                    .expect("errors_handle just initialised");
                write_error_line(
                    handle,
                    i + 1,
                    entry,
                    "claude_open_or_translator",
                    &e.to_string(),
                )?;
                eprintln!("[{}/{}] {} · FAILED: {}", i + 1, total, entry.category, e);
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
        cfg.translator.style.as_str().to_string(),
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
    eprintln!(
        "bench run complete: {}/{} succeeded · ZH input {} vs EN · report: {}",
        summary.n_succeeded,
        total,
        dstr,
        md_path.display()
    );
    if opts.json {
        println!("{}", serde_json::to_string(&summary)?);
    }
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
    eprintln!(
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

#[allow(clippy::too_many_arguments)]
async fn run_coding_eval(
    cfg: &SigoConfig,
    opts: &RunOptions,
    backend_kind: BackendKind,
    translator_builder: &TranslatorBuilder,
    backend_builder: &BackendBuilder,
) -> Result<()> {
    use std::time::Duration;

    if opts.samples != 1 {
        anyhow::bail!("--samples > 1 (pass@k) is not yet implemented; use --samples 1");
    }

    // Fail fast if the executor is missing — otherwise every task silently scores
    // RuntimeError and the report looks like genuine all-fail measurement data.
    let python_ok = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !python_ok {
        anyhow::bail!(
            "python3 not found on PATH — required for `--eval coding` (run `sigo doctor` to check)"
        );
    }

    let run_id = build_run_id(opts.label.as_deref());
    let out_dir = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| default_run_dir(&run_id));
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create out_dir {}", out_dir.display()))?;

    let tasks = load_coding_corpus(opts.corpus_path.as_deref())
        .map_err(|e| anyhow::anyhow!("coding corpus load: {e}"))?;
    let tasks: Vec<_> = match opts.limit {
        Some(n) => tasks.into_iter().take(n).collect(),
        None => tasks,
    };
    if tasks.is_empty() {
        anyhow::bail!("coding corpus is empty after applying --limit");
    }

    let tokenizer: Arc<dyn Tokenizer> =
        Arc::new(TokenizerProxy::new().context("failed to initialize o200k_base proxy tokenizer")?);
    // Rolling audit log: the orchestrator appends each ZH TurnRecord here, same as a
    // normal bench run. The structured benchmark output is eval_report.{md,csv} below.
    let sink: Arc<dyn BenchmarkSink> =
        Arc::new(JsonlSink::open(cfg.resolved_log_path()).context("failed to open benchmark log")?);
    let judge = OllamaJudge::new(
        &cfg.translator.endpoint,
        &cfg.translator.model,
        Duration::from_secs(cfg.translator.timeout_seconds),
    );
    let exec_timeout = Duration::from_secs(10);

    let total = tasks.len();
    eprintln!(
        "sigo bench run --eval coding · run_id={run_id} · backend={} · {total} tasks",
        cfg.claude.backend
    );

    let mut evals: Vec<TaskEval> = Vec::with_capacity(total);
    let mut n_failed = 0usize;
    for (i, task) in tasks.iter().enumerate() {
        let translator = translator_builder();
        let backend = backend_builder()
            .with_context(|| format!("build backend for task {} ({})", i + 1, task.task_id))?;
        let mut orch = Orchestrator::new(
            OrchestratorConfig {
                backend_kind,
                claude_model: cfg.claude.model.clone(),
                translator_model: cfg.translator.model.clone(),
                control_mode: ControlMode::Full,
            },
            translator.clone(),
            backend,
            tokenizer.clone(),
            sink.clone(),
        );

        let mut out = SilentSink;
        let record = match orch.run_turn(&task.prompt, &mut out).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[{}/{}] {} · FAILED: {e}", i + 1, total, task.task_id);
                n_failed += 1;
                continue;
            }
        };

        // ZH arm answer = raw Chinese response (code preserved untranslated).
        let zh_outcome = evaluate_answer(
            &record.chinese_response,
            &task.test,
            &task.entry_point,
            exec_timeout,
        )
        .await;
        // EN arm answer = the English control run's response.
        let en_answer = match record.english_control_run.as_ref() {
            Some(c) => c.english_response.as_str(),
            None => {
                eprintln!("[{}/{}] {} · WARN: no English control run (Full mode expected); EN arm scores no_code", i + 1, total, task.task_id);
                ""
            }
        };
        let en_outcome =
            evaluate_answer(en_answer, &task.test, &task.entry_point, exec_timeout).await;

        let zh_in_proxy = tokenizer.count_tokens(&record.chinese_prompt).unwrap_or(0);
        let en_in_proxy = tokenizer.count_tokens(&task.prompt).unwrap_or(0);

        let zh_cost = ArmCost {
            input: record.chinese_prompt_tokens_reported.unwrap_or(0),
            output: record.chinese_response_tokens_reported.unwrap_or(0),
            cache_read: record.cache_read_tokens_reported.unwrap_or(0),
            cache_write: record.cache_write_tokens_reported.unwrap_or(0),
        };
        let en_cost = record
            .english_control_run
            .as_ref()
            .map(|c| ArmCost {
                input: c.prompt_tokens_reported,
                output: c.response_tokens_reported,
                cache_read: c.cache_read_tokens_reported.unwrap_or(0),
                cache_write: c.cache_write_tokens_reported.unwrap_or(0),
            })
            .unwrap_or_default();

        let fidelity =
            roundtrip_fidelity(&translator, &judge, &task.prompt, &record.chinese_prompt).await;

        eprintln!(
            "[{}/{}] {} · en={} zh={} · zh-in={} en-in={}",
            i + 1,
            total,
            task.task_id,
            en_outcome.label(),
            zh_outcome.label(),
            zh_cost.input,
            en_cost.input
        );

        evals.push(TaskEval {
            task_id: task.task_id.clone(),
            category: task.category.clone(),
            en: ArmEval {
                outcome: en_outcome,
                cost: en_cost,
                proxy_in: en_in_proxy,
            },
            zh: ArmEval {
                outcome: zh_outcome,
                cost: zh_cost,
                proxy_in: zh_in_proxy,
            },
            fidelity,
        });
    }

    if evals.is_empty() {
        anyhow::bail!("no tasks produced a usable record");
    }
    let summary = summarize_eval(&evals, &cfg.pricing, cfg.benchmark.bootstrap_seed);
    let md = build_eval_markdown(
        &run_id,
        &cfg.claude.backend,
        &cfg.claude.model,
        cfg.translator.style.as_str(),
        &summary,
    );
    let csv = build_eval_csv(&evals, &cfg.pricing);
    let md_path = out_dir.join("eval_report.md");
    std::fs::write(&md_path, md).with_context(|| format!("write {}", md_path.display()))?;
    std::fs::write(out_dir.join("eval_report.csv"), csv).context("write eval_report.csv")?;

    eprintln!(
        "coding eval complete: {} scored, {} failed/skipped of {} · EN pass {}/{} · ZH pass {}/{} · report: {}",
        evals.len(), n_failed, total,
        summary.en_pass.passes, summary.en_pass.n,
        summary.zh_pass.passes, summary.zh_pass.n, md_path.display()
    );
    if opts.json {
        println!("{}", serde_json::to_string(&summary)?);
    }
    Ok(())
}

struct SilentSink;
impl OutputSink for SilentSink {
    fn write(&mut self, _: &str) {}
}
