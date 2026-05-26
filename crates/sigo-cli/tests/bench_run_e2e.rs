use std::sync::Arc;

use sigo_cli::commands::bench_run::{
    run_with_builders, BackendBuilder, RunOptions, TranslatorBuilder,
};
use sigo_core::{
    BackendKind, ClaudeBackend, FakeBackend, FakeTranslator, SigoConfig, Translator, Usage,
};
use tempfile::TempDir;

fn make_config(jsonl_path: std::path::PathBuf) -> SigoConfig {
    let mut c = SigoConfig::default();
    c.benchmark.log_path = Some(jsonl_path);
    c.claude.backend = "api".into();
    c
}

#[tokio::test]
async fn happy_path_three_prompts_two_categories() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("turns.jsonl");
    let out_dir = tmp.path().join("run-out");

    // Two-category mini corpus.
    let corpus_path = tmp.path().join("corpus.jsonl");
    std::fs::write(
        &corpus_path,
        concat!(
            r#"{"category":"factual","prompt":"What is HSTS?"}"#,
            "\n",
            r#"{"category":"factual","prompt":"What does TCP stand for?"}"#,
            "\n",
            r#"{"category":"prose","prompt":"Write a haiku about logs."}"#,
            "\n",
        ),
    )
    .unwrap();

    let translator = Arc::new(FakeTranslator::new());
    translator.add_en_to_zh("What is HSTS?", "什么是HSTS？");
    translator.add_en_to_zh("What does TCP stand for?", "TCP是什么？");
    translator.add_en_to_zh("Write a haiku about logs.", "写一首关于日志的俳句。");
    // ZH→EN translations triggered by the streamed response segments:
    translator.add_zh_to_en("响应。", "Response.");

    let backend = Arc::new(FakeBackend::new());
    // For each prompt the runner does: one ZH call + one EN-control call. So 6 enqueues for 3 prompts.
    for input in &[5u32, 6, 7] {
        backend.enqueue_simple(
            "响应。",
            Usage {
                input_tokens: *input,
                output_tokens: 10,
                cache_read: Some(100),
                cache_write: Some(20),
            },
        );
        backend.enqueue_simple(
            "EN-response.",
            Usage {
                input_tokens: input + 4,
                output_tokens: 12,
                cache_read: Some(100),
                cache_write: Some(20),
            },
        );
    }

    let translator_clone = translator.clone();
    let backend_clone = backend.clone();
    let translator_builder: TranslatorBuilder =
        Arc::new(move || translator_clone.clone() as Arc<dyn Translator>);
    let backend_builder: BackendBuilder =
        Arc::new(move || Ok(backend_clone.clone() as Arc<dyn ClaudeBackend>));

    let cfg = make_config(jsonl.clone());
    let opts = RunOptions {
        corpus_path: Some(corpus_path),
        label: Some("e2e".into()),
        limit: None,
        out_dir: Some(out_dir.clone()),
    };
    run_with_builders(&cfg, opts, BackendKind::Api, translator_builder, backend_builder)
        .await
        .expect("runner should succeed");

    // 3 records in the rolling JSONL.
    let jsonl_str = std::fs::read_to_string(&jsonl).unwrap();
    let line_count = jsonl_str.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 3, "expected 3 records in rolling jsonl");

    // report.md exists and contains the headline and category breakdown.
    let md = std::fs::read_to_string(out_dir.join("report.md")).unwrap();
    assert!(md.contains("## Headline"));
    assert!(md.contains("## Per-category"));
    assert!(md.contains("| factual |"));
    assert!(md.contains("| prose |"));

    // report.csv has 3 data rows + 1 header.
    let csv = std::fs::read_to_string(out_dir.join("report.csv")).unwrap();
    let csv_lines: Vec<&str> = csv.lines().collect();
    assert_eq!(csv_lines.len(), 4);
    assert!(csv_lines[0].starts_with("run_id,prompt_index,"));

    // errors.jsonl should NOT exist — no failures.
    assert!(!out_dir.join("errors.jsonl").exists());
}

#[tokio::test]
async fn translator_failure_skips_prompt_and_logs_to_errors_jsonl() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("turns.jsonl");
    let out_dir = tmp.path().join("run-out");

    let corpus_path = tmp.path().join("corpus.jsonl");
    std::fs::write(&corpus_path, concat!(
        r#"{"category":"factual","prompt":"first"}"#, "\n",
        r#"{"category":"factual","prompt":"second"}"#, "\n",
        r#"{"category":"factual","prompt":"third"}"#, "\n",
    )).unwrap();

    // Use strict mode: "second" has no EN→ZH mapping, so translate() returns Err.
    let translator = Arc::new(FakeTranslator::new_strict());
    translator.add_en_to_zh("first", "第一");
    translator.add_en_to_zh("third", "第三");
    translator.add_zh_to_en("响应。", "Response.");

    let backend = Arc::new(FakeBackend::new());
    // Only 2 prompts succeed, each does ZH + EN-control = 4 enqueues total.
    for _ in 0..2 {
        backend.enqueue_simple("响应。", Usage { input_tokens: 5, output_tokens: 10, cache_read: Some(100), cache_write: Some(20) });
        backend.enqueue_simple("EN-response.", Usage { input_tokens: 8, output_tokens: 12, cache_read: Some(100), cache_write: Some(20) });
    }

    let translator_clone = translator.clone();
    let backend_clone = backend.clone();
    let translator_builder: TranslatorBuilder = Arc::new(move || translator_clone.clone() as Arc<dyn Translator>);
    let backend_builder: BackendBuilder = Arc::new(move || Ok(backend_clone.clone() as Arc<dyn ClaudeBackend>));

    let cfg = make_config(jsonl.clone());
    let opts = RunOptions {
        corpus_path: Some(corpus_path),
        label: Some("tx-fail".into()),
        limit: None,
        out_dir: Some(out_dir.clone()),
    };
    run_with_builders(&cfg, opts, BackendKind::Api, translator_builder, backend_builder)
        .await
        .expect("runner should not abort on per-prompt failure");

    // 2 succeeded → 2 records in rolling JSONL.
    let line_count = std::fs::read_to_string(&jsonl).unwrap()
        .lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 2);

    // errors.jsonl exists with exactly one entry for the failed prompt.
    let errors = std::fs::read_to_string(out_dir.join("errors.jsonl")).unwrap();
    let err_lines: Vec<&str> = errors.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(err_lines.len(), 1);
    assert!(err_lines[0].contains("\"prompt\":\"second\""));

    // The report still gets written and shows the right counts.
    let md = std::fs::read_to_string(out_dir.join("report.md")).unwrap();
    assert!(md.contains("attempted=3 succeeded=2 incomplete=0 failed=1"));
}

#[tokio::test]
async fn mid_stream_claude_error_marks_incomplete_not_failed() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("turns.jsonl");
    let out_dir = tmp.path().join("run-out");

    let corpus_path = tmp.path().join("corpus.jsonl");
    std::fs::write(&corpus_path, concat!(
        r#"{"category":"factual","prompt":"first"}"#, "\n",
        r#"{"category":"factual","prompt":"second"}"#, "\n",
    )).unwrap();

    let translator = Arc::new(FakeTranslator::new());
    translator.add_en_to_zh("first", "第一");
    translator.add_en_to_zh("second", "第二");
    translator.add_zh_to_en("响应。", "Response.");
    translator.add_zh_to_en("响应", "Response");

    let backend = Arc::new(FakeBackend::new());
    // Prompt 1: clean ZH + clean EN control.
    backend.enqueue_simple("响应。", Usage { input_tokens: 5, output_tokens: 10, cache_read: Some(100), cache_write: Some(20) });
    backend.enqueue_simple("EN-resp.", Usage { input_tokens: 8, output_tokens: 12, cache_read: Some(100), cache_write: Some(20) });
    // Prompt 2: ZH mid-stream error (incomplete) + clean EN control.
    backend.enqueue_error_after_chunk("响应", "simulated mid-stream drop");
    backend.enqueue_simple("EN-resp.", Usage { input_tokens: 9, output_tokens: 11, cache_read: Some(100), cache_write: Some(20) });

    let translator_clone = translator.clone();
    let backend_clone = backend.clone();
    let translator_builder: TranslatorBuilder = Arc::new(move || translator_clone.clone() as Arc<dyn Translator>);
    let backend_builder: BackendBuilder = Arc::new(move || Ok(backend_clone.clone() as Arc<dyn ClaudeBackend>));

    let cfg = make_config(jsonl.clone());
    let opts = RunOptions {
        corpus_path: Some(corpus_path),
        label: Some("midstream".into()),
        limit: None,
        out_dir: Some(out_dir.clone()),
    };
    run_with_builders(&cfg, opts, BackendKind::Api, translator_builder, backend_builder)
        .await
        .expect("runner should not abort on incomplete turn");

    // Both records should be in the rolling JSONL (incomplete records are still recorded).
    let line_count = std::fs::read_to_string(&jsonl).unwrap()
        .lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 2);

    // errors.jsonl should NOT exist (mid-stream errors are recorded in-band, not as failures).
    assert!(!out_dir.join("errors.jsonl").exists());

    let md = std::fs::read_to_string(out_dir.join("report.md")).unwrap();
    assert!(md.contains("attempted=2 succeeded=1 incomplete=1 failed=0"));
}
