use sigo_core::{
    build_eval_csv, build_eval_markdown, evaluate_answer, summarize_eval, ArmCost, ArmEval,
    Outcome, PricingConfig, TaskEval,
};
use std::time::Duration;

fn arm(o: Outcome, input: u32, output: u32, proxy: u32) -> ArmEval {
    ArmEval {
        outcome: o,
        cost: ArmCost {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
        },
        proxy_in: proxy,
    }
}

#[test]
fn layered_report_reflects_en_advantage() {
    let tasks = vec![
        TaskEval {
            task_id: "HumanEval/0".into(),
            category: "coding-verifiable".into(),
            en: arm(Outcome::Pass, 100, 200, 90),
            zh: arm(Outcome::AssertFail, 140, 260, 70),
            fidelity: Some(7),
        },
        TaskEval {
            task_id: "HumanEval/1".into(),
            category: "coding-verifiable".into(),
            en: arm(Outcome::Pass, 110, 210, 100),
            zh: arm(Outcome::Pass, 150, 250, 80),
            fidelity: Some(9),
        },
    ];
    let s = summarize_eval(&tasks, &PricingConfig::default(), 0xC0DE);
    assert_eq!(s.en_pass.passes, 2);
    assert_eq!(s.zh_pass.passes, 1);
    assert!(
        s.reported_input.mean_delta_pct > 0.0,
        "ZH should cost more reported input here"
    );
    assert!(s.zh_cost_per_pass > s.en_cost_per_pass);

    let md = build_eval_markdown("rid", "claude-code", "claude-sonnet-4-6", &s);
    assert!(md.contains("ZH win-rate"));
    assert!(md.contains("$ / passing task"));
    let csv = build_eval_csv(&tasks, &PricingConfig::default());
    assert_eq!(csv.lines().count(), 5); // header + 2×2
}

#[tokio::test]
async fn evaluate_answer_scores_real_pair() {
    let py = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !py {
        eprintln!("skip: no python3");
        return;
    }
    let answer = "```python\ndef add(a, b):\n    return a + b\n```";
    let test = "def check(candidate):\n    assert candidate(2, 3) == 5\n";
    assert_eq!(
        evaluate_answer(answer, test, "add", Duration::from_secs(10)).await,
        Outcome::Pass
    );
}

// ---------------------------------------------------------------------------
// Test 2: exercise run_coding_eval end-to-end with fakes
// ---------------------------------------------------------------------------
//
// Design notes for the two-arm fake-response setup:
//
// The orchestrator's run_turn (ControlMode::Full) does:
//   1. tokio::spawn(run_english_control(backend.clone(), ...))  — concurrent EN call
//   2. backend.stream_turn(...chinese_prompt...)                — ZH call
//
// Both calls drain from the same FakeBackend FIFO queue, and the spawn may race
// with the ZH call.  Because we give BOTH arms of each task the IDENTICAL answer
// (same code string, same Usage), the test result is independent of which queue
// slot is consumed by which arm — both pass or both fail by the same logic.
// This makes the test deterministic regardless of tokio scheduling order.
//
// Enqueue order: for each task, enqueue the correct/incorrect answer TWICE
// (once for the ZH call, once for the EN control call).

#[tokio::test]
async fn eval_mode_writes_report_with_fakes() {
    use sigo_cli::commands::bench_run::{
        run_with_builders, BackendBuilder, RunOptions, TranslatorBuilder,
    };
    use sigo_core::{ClaudeBackend, FakeBackend, FakeTranslator, SigoConfig, Translator, Usage};
    use std::sync::Arc;
    use tempfile::TempDir;

    // Skip if python3 is unavailable — code execution requires it.
    let py = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !py {
        eprintln!("skip: no python3");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("turns.jsonl");
    let out_dir = tmp.path().join("eval-out");

    // ── Corpus ──────────────────────────────────────────────────────────────
    // Two coding tasks:
    //   t/pass — add(a,b): correct solution will be returned by the fake backend.
    //   t/fail — sub(a,b): wrong solution (add instead of sub) will be returned.
    let corpus_path = tmp.path().join("corpus.jsonl");
    std::fs::write(
        &corpus_path,
        concat!(
            "{\"task_id\":\"t/pass\",\"prompt\":\"def add(a, b):\\n    \\\"\\\"\\\"add\\\"\\\"\\\"\\n\",",
            "\"entry_point\":\"add\",\"test\":\"def check(candidate):\\n    assert candidate(2,3)==5\\n\"}\n",
            "{\"task_id\":\"t/fail\",\"prompt\":\"def sub(a, b):\\n    \\\"\\\"\\\"sub\\\"\\\"\\\"\\n\",",
            "\"entry_point\":\"sub\",\"test\":\"def check(candidate):\\n    assert candidate(5,3)==2\\n\"}\n",
        ),
    )
    .unwrap();

    // ── Translator fakes ─────────────────────────────────────────────────────
    // Lenient mode: unknown inputs produce a [mock …] placeholder, so we only
    // need to register the prompt strings we actually send.
    let translator = Arc::new(FakeTranslator::new());
    // EN→ZH for the two prompts.
    translator.add_en_to_zh(
        "def add(a, b):\n    \"\"\"add\"\"\"\n",
        "def add(a, b):  # 加法\n    pass\n",
    );
    translator.add_en_to_zh(
        "def sub(a, b):\n    \"\"\"sub\"\"\"\n",
        "def sub(a, b):  # 减法\n    pass\n",
    );
    // The code-block responses the fake backend returns contain no Chinese
    // prose sentences, so the sentence buffer will emit them as Passthrough
    // segments (no ZH→EN translation calls).  No add_zh_to_en registrations
    // are required; lenient mode handles any stray calls gracefully.

    // ── Backend fakes ────────────────────────────────────────────────────────
    // For each task the orchestrator makes TWO backend calls in ControlMode::Full:
    //   • the ZH call  (stream_turn on the chinese prompt)
    //   • the EN control call (run_english_control, spawned concurrently)
    //
    // We enqueue each answer TWICE so that regardless of which call drains which
    // queue slot first, both arms receive the same answer and produce the same
    // Outcome.  This makes the assertions independent of tokio scheduling order.
    let backend = Arc::new(FakeBackend::new());

    let pass_answer = "```python\ndef add(a, b):\n    return a + b\n```";
    let fail_answer = "```python\ndef sub(a, b):\n    return a + b\n```"; // wrong: adds instead of subtracts

    let mk_usage = || Usage {
        input_tokens: 10,
        output_tokens: 20,
        cache_read: Some(0),
        cache_write: Some(0),
    };

    // t/pass — enqueue the correct solution twice (ZH arm + EN control arm).
    backend.enqueue_simple(pass_answer, mk_usage());
    backend.enqueue_simple(pass_answer, mk_usage());

    // t/fail — enqueue the wrong solution twice.
    backend.enqueue_simple(fail_answer, mk_usage());
    backend.enqueue_simple(fail_answer, mk_usage());

    // ── Builders ─────────────────────────────────────────────────────────────
    let translator_clone = translator.clone();
    let backend_clone = backend.clone();
    let translator_builder: TranslatorBuilder =
        Arc::new(move || translator_clone.clone() as Arc<dyn Translator>);
    let backend_builder: BackendBuilder =
        Arc::new(move || Ok(backend_clone.clone() as Arc<dyn ClaudeBackend>));

    // ── Config ───────────────────────────────────────────────────────────────
    let mut cfg = SigoConfig::default();
    cfg.benchmark.log_path = Some(jsonl.clone());
    cfg.claude.backend = "api".into();

    let opts = RunOptions {
        corpus_path: Some(corpus_path),
        label: None,
        limit: None,
        out_dir: Some(out_dir.clone()),
        eval: Some("coding".into()),
        samples: 1,
    };

    run_with_builders(&cfg, opts, translator_builder, backend_builder)
        .await
        .expect("coding eval runner should succeed");

    // ── Assertions ───────────────────────────────────────────────────────────
    let md_path = out_dir.join("eval_report.md");
    let csv_path = out_dir.join("eval_report.csv");

    assert!(md_path.exists(), "eval_report.md must exist");
    assert!(csv_path.exists(), "eval_report.csv must exist");

    // CSV: header + 2 tasks × 2 arms = 5 lines.
    let csv = std::fs::read_to_string(&csv_path).unwrap();
    let csv_lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        csv_lines.len(),
        5,
        "expected header + 4 data rows; got: {:?}",
        csv_lines
    );
    assert!(
        csv_lines[0].starts_with("task_id,category,arm,outcome"),
        "unexpected CSV header: {}",
        csv_lines[0]
    );

    // Markdown: must contain the EN pass / ZH win-rate table.
    let md = std::fs::read_to_string(&md_path).unwrap();
    assert!(
        md.contains("EN pass") || md.contains("| EN |"),
        "markdown should contain EN pass row; got:\n{md}"
    );

    // t/pass passes both arms; t/fail fails both.
    // EN pass count must be exactly 1 (out of 2 tasks).
    // The markdown renders it as "| EN | 1 | 2 |..." — check the numeric values.
    assert!(
        md.contains("| EN | 1 |") || md.contains("EN | 1 |"),
        "expected EN pass count of 1 in markdown; got:\n{md}"
    );
}
