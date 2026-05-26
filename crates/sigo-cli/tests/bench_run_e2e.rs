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
