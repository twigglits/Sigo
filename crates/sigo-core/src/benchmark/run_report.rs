use crate::benchmark::TurnRecord;
use crate::conversation::BackendKind;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Aggregated paired-comparison numbers for one bench run.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub wall_ms: u64,
    pub backend: BackendKind,
    pub claude_model: String,
    pub translator_model: String,
    pub corpus_source: String,
    pub n_attempted: usize,
    pub n_succeeded: usize,
    pub n_incomplete: usize,
    pub n_failed: usize,

    pub mean_zh_input_reported: f64,
    pub mean_en_input_reported: f64,
    pub mean_zh_total_input: f64,
    pub mean_en_total_input: f64,
    pub mean_zh_output_reported: f64,
    pub mean_en_output_reported: f64,
    pub mean_zh_prompt_local: f64,
    pub mean_en_prompt_local: f64,
    pub mean_turn_total_ms: f64,

    pub per_category: BTreeMap<String, CategoryStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryStats {
    pub n: usize,
    pub mean_en_input: f64,
    pub mean_zh_input: f64,
    pub mean_en_total: f64,
    pub mean_zh_total: f64,
    pub mean_en_output: f64,
    pub mean_zh_output: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub summary: RunSummary,
    /// (turn_record, category) — category is carried separately because TurnRecord
    /// has no category field; the runner records it alongside.
    pub rows: Vec<(TurnRecord, String)>,
}

/// Compute deltas as a percent of the EN side: `(zh - en) / en * 100`.
/// Returns `None` if `en == 0.0`, so callers can decide how to render N/A.
pub fn delta_pct(zh: f64, en: f64) -> Option<f64> {
    if en == 0.0 { None } else { Some((zh - en) / en * 100.0) }
}

fn verdict(delta: Option<f64>) -> &'static str {
    match delta {
        None => "n/a",
        Some(d) if d.abs() < 5.0 => "wash",
        Some(d) if d < 0.0 => "ZH wins",
        Some(_) => "EN wins",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn summarize_run(
    run_id: String,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    backend: BackendKind,
    claude_model: String,
    translator_model: String,
    corpus_source: String,
    n_attempted: usize,
    n_failed: usize,
    rows: &[(TurnRecord, String)],
) -> RunSummary {
    let n = rows.len() as f64;
    let n_succeeded = rows.iter().filter(|(r, _)| !r.incomplete).count();
    let n_incomplete = rows.iter().filter(|(r, _)| r.incomplete).count();

    let mean = |sel: &dyn Fn(&TurnRecord) -> f64| -> f64 {
        if rows.is_empty() { 0.0 } else { rows.iter().map(|(r, _)| sel(r)).sum::<f64>() / n }
    };

    let zh_input  = |r: &TurnRecord| r.chinese_prompt_tokens_reported.unwrap_or(0) as f64;
    let zh_cr     = |r: &TurnRecord| r.cache_read_tokens_reported.unwrap_or(0) as f64;
    let zh_cw     = |r: &TurnRecord| r.cache_write_tokens_reported.unwrap_or(0) as f64;
    let zh_output = |r: &TurnRecord| r.chinese_response_tokens_reported.unwrap_or(0) as f64;
    let en_input  = |r: &TurnRecord| r.english_control_run.as_ref().map(|c| c.prompt_tokens_reported as f64).unwrap_or(0.0);
    let en_cr     = |r: &TurnRecord| r.english_control_run.as_ref().and_then(|c| c.cache_read_tokens_reported).unwrap_or(0) as f64;
    let en_cw     = |r: &TurnRecord| r.english_control_run.as_ref().and_then(|c| c.cache_write_tokens_reported).unwrap_or(0) as f64;
    let en_output = |r: &TurnRecord| r.english_control_run.as_ref().map(|c| c.response_tokens_reported as f64).unwrap_or(0.0);

    let zh_total = |r: &TurnRecord| zh_input(r) + zh_cr(r) + zh_cw(r);
    let en_total = |r: &TurnRecord| en_input(r) + en_cr(r) + en_cw(r);

    let mut per_category: BTreeMap<String, Vec<&TurnRecord>> = BTreeMap::new();
    for (r, cat) in rows {
        per_category.entry(cat.clone()).or_default().push(r);
    }
    let per_category = per_category.into_iter().map(|(cat, recs)| {
        let nc = recs.len() as f64;
        let cmean = |sel: &dyn Fn(&TurnRecord) -> f64|
            recs.iter().map(|r| sel(r)).sum::<f64>() / nc;
        let stats = CategoryStats {
            n: recs.len(),
            mean_en_input:  cmean(&en_input),
            mean_zh_input:  cmean(&zh_input),
            mean_en_total:  cmean(&en_total),
            mean_zh_total:  cmean(&zh_total),
            mean_en_output: cmean(&en_output),
            mean_zh_output: cmean(&zh_output),
        };
        (cat, stats)
    }).collect();

    RunSummary {
        run_id,
        started_at,
        finished_at,
        wall_ms: (finished_at - started_at).num_milliseconds().max(0) as u64,
        backend,
        claude_model,
        translator_model,
        corpus_source,
        n_attempted,
        n_succeeded,
        n_incomplete,
        n_failed,
        mean_zh_input_reported: mean(&zh_input),
        mean_en_input_reported: mean(&en_input),
        mean_zh_total_input:    mean(&zh_total),
        mean_en_total_input:    mean(&en_total),
        mean_zh_output_reported: mean(&zh_output),
        mean_en_output_reported: mean(&en_output),
        mean_zh_prompt_local: mean(&(|r| r.chinese_prompt_tokens_local as f64)),
        mean_en_prompt_local: mean(&(|r| r.english_prompt_tokens_local as f64)),
        mean_turn_total_ms:   mean(&(|r| r.turn_total_ms as f64)),
        per_category,
    }
}

pub fn build_markdown(report: &RunReport) -> String {
    let s = &report.summary;
    let mut out = String::new();

    let _ = writeln!(out, "# Sigo bench run — `{}`", s.run_id);
    let _ = writeln!(out);
    let _ = writeln!(out, "- started: `{}`", s.started_at.to_rfc3339());
    let _ = writeln!(out, "- finished: `{}` (wall {} ms)", s.finished_at.to_rfc3339(), s.wall_ms);
    let _ = writeln!(out, "- backend: `{:?}`  · claude_model: `{}`  · translator_model: `{}`", s.backend, s.claude_model, s.translator_model);
    let _ = writeln!(out, "- corpus: `{}`", s.corpus_source);
    let _ = writeln!(out, "- attempted={} succeeded={} incomplete={} failed={}", s.n_attempted, s.n_succeeded, s.n_incomplete, s.n_failed);
    let _ = writeln!(out, "- control_mode: `full`");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Headline");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Metric | EN | ZH | Δ% | Verdict |");
    let _ = writeln!(out, "|---|---:|---:|---:|---|");
    let row = |label: &str, en: f64, zh: f64, out: &mut String| {
        let d = delta_pct(zh, en);
        let dstr = d.map(|v| format!("{:+.1}%", v)).unwrap_or_else(|| "n/a".into());
        let _ = writeln!(out, "| {label} | {en:.1} | {zh:.1} | {dstr} | {} |", verdict(d));
    };
    row("reported input tokens (uncached)", s.mean_en_input_reported, s.mean_zh_input_reported, &mut out);
    row("total input (input + cache_read + cache_write)", s.mean_en_total_input, s.mean_zh_total_input, &mut out);
    row("reported output tokens", s.mean_en_output_reported, s.mean_zh_output_reported, &mut out);
    row("local-tokenizer prompt count", s.mean_en_prompt_local, s.mean_zh_prompt_local, &mut out);
    let _ = writeln!(out);
    let _ = writeln!(out, "- mean wall per turn: {:.0} ms", s.mean_turn_total_ms);
    let _ = writeln!(out);

    let _ = writeln!(out, "## Per-category");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Category | N | EN-in | ZH-in | Δ% | EN-tot | ZH-tot | Δ% | EN-out | ZH-out | Δ% |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (cat, cs) in &s.per_category {
        let di = delta_pct(cs.mean_zh_input, cs.mean_en_input).map(|v| format!("{:+.1}%", v)).unwrap_or_else(|| "n/a".into());
        let dt = delta_pct(cs.mean_zh_total, cs.mean_en_total).map(|v| format!("{:+.1}%", v)).unwrap_or_else(|| "n/a".into());
        let do_ = delta_pct(cs.mean_zh_output, cs.mean_en_output).map(|v| format!("{:+.1}%", v)).unwrap_or_else(|| "n/a".into());
        let _ = writeln!(out, "| {cat} | {} | {:.1} | {:.1} | {di} | {:.1} | {:.1} | {dt} | {:.1} | {:.1} | {do_} |",
            cs.n, cs.mean_en_input, cs.mean_zh_input, cs.mean_en_total, cs.mean_zh_total, cs.mean_en_output, cs.mean_zh_output);
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Caveats");
    let _ = writeln!(out);
    if matches!(s.backend, BackendKind::ClaudeCode) {
        let _ = writeln!(out, "- Reported `input_tokens` for the `claude-code` backend excludes cached system-prompt scaffolding (~tens of thousands of tokens per turn). The 'Total input' row above is the fair comparison.");
    }
    if s.n_failed > 0 {
        let _ = writeln!(out, "- {} prompts failed in translation or Claude. See `errors.jsonl` in the run directory.", s.n_failed);
    }
    let _ = writeln!(out, "- Sample size is {} prompts; treat means as point estimates. The CSV is the input to your stats package of choice.", s.n_attempted);
    let _ = writeln!(out, "- ZH responses were translated back to EN by the local translator; the EN you'd read is not a like-for-like answer match to the EN control's response. The token-cost comparison is unaffected by this, but a quality comparison is not in scope.");

    out
}

pub fn build_csv(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str("run_id,prompt_index,category,prompt,");
    out.push_str("zh_input_reported,zh_output_reported,zh_cache_read,zh_cache_write,zh_total_input,");
    out.push_str("en_input_reported,en_output_reported,en_cache_read,en_cache_write,en_total_input,");
    out.push_str("zh_prompt_local,en_prompt_local,zh_response_local,");
    out.push_str("delta_input_pct,delta_total_input_pct,delta_output_pct,");
    out.push_str("translation_in_ms,translation_out_ms_total,claude_total_ms,turn_total_ms,");
    out.push_str("incomplete,errors\n");

    for (i, (r, cat)) in report.rows.iter().enumerate() {
        let ec = r.english_control_run.as_ref();
        let zh_input = r.chinese_prompt_tokens_reported.unwrap_or(0);
        let zh_cr = r.cache_read_tokens_reported.unwrap_or(0);
        let zh_cw = r.cache_write_tokens_reported.unwrap_or(0);
        let zh_total = zh_input + zh_cr + zh_cw;
        let en_input = ec.map(|c| c.prompt_tokens_reported).unwrap_or(0);
        let en_cr = ec.and_then(|c| c.cache_read_tokens_reported).unwrap_or(0);
        let en_cw = ec.and_then(|c| c.cache_write_tokens_reported).unwrap_or(0);
        let en_total = en_input + en_cr + en_cw;

        let di = if en_input > 0 { format!("{:+.2}", (zh_input as f64 - en_input as f64) / en_input as f64 * 100.0) } else { "".into() };
        let dt = if en_total > 0 { format!("{:+.2}", (zh_total as f64 - en_total as f64) / en_total as f64 * 100.0) } else { "".into() };
        let zh_out = r.chinese_response_tokens_reported.unwrap_or(0);
        let en_out = ec.map(|c| c.response_tokens_reported).unwrap_or(0);
        let do_ = if en_out > 0 { format!("{:+.2}", (zh_out as f64 - en_out as f64) / en_out as f64 * 100.0) } else { "".into() };

        let _ = writeln!(out,
            "{run},{idx},{cat},{prompt},{zh_input},{zh_out},{zh_cr},{zh_cw},{zh_total},{en_input},{en_out},{en_cr},{en_cw},{en_total},{zhpl},{enpl},{zhrl},{di},{dt},{do_},{tin},{toutsum},{cmt},{ttm},{inc},{errs}",
            run = csv_quote(&report.summary.run_id),
            idx = i,
            cat = csv_quote(cat),
            prompt = csv_quote(&r.english_prompt),
            zhpl = r.chinese_prompt_tokens_local,
            enpl = r.english_prompt_tokens_local,
            zhrl = r.chinese_response_tokens_local,
            tin = r.translation_in_ms,
            toutsum = r.translation_out_ms_total,
            cmt = r.claude_total_ms,
            ttm = r.turn_total_ms,
            inc = r.incomplete,
            errs = csv_quote(&r.turn_errors.join(";")),
        );
    }
    out
}

fn csv_quote(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::EnglishControlRun;
    use chrono::TimeZone;

    #[allow(clippy::too_many_arguments)]
    fn rec(
        category_marker_unused: &str,
        en_local: u32,
        zh_local: u32,
        zh_input_rep: u32,
        zh_out_rep: u32,
        zh_cr: u32,
        zh_cw: u32,
        en_input_rep: u32,
        en_out_rep: u32,
        en_cr: u32,
        en_cw: u32,
        incomplete: bool,
    ) -> TurnRecord {
        let _ = category_marker_unused;
        TurnRecord {
            schema_version: 2,
            session_id: uuid::Uuid::nil(),
            turn_index: 0,
            timestamp: Utc::now(),
            backend: BackendKind::Api,
            claude_model: "claude-sonnet-4-6".into(),
            translator_model: "qwen3".into(),
            english_prompt: "x".into(),
            chinese_prompt: "y".into(),
            chinese_response: "".into(),
            english_response: "".into(),
            english_prompt_tokens_local: en_local,
            chinese_prompt_tokens_local: zh_local,
            chinese_response_tokens_local: 0,
            chinese_prompt_tokens_reported: Some(zh_input_rep),
            chinese_response_tokens_reported: Some(zh_out_rep),
            cache_read_tokens_reported: Some(zh_cr),
            cache_write_tokens_reported: Some(zh_cw),
            chinese_cumulative_prompt_tokens_local: zh_local,
            english_cumulative_prompt_tokens_local: en_local,
            english_control_run: Some(EnglishControlRun {
                english_response: "".into(),
                prompt_tokens_reported: en_input_rep,
                response_tokens_reported: en_out_rep,
                cache_read_tokens_reported: Some(en_cr),
                cache_write_tokens_reported: Some(en_cw),
                duration_ms: 0,
            }),
            incomplete,
            turn_errors: vec![],
            translation_in_ms: 0,
            translation_out_ms_total: 0,
            translation_out_calls: 0,
            claude_ttft_ms: 0,
            claude_total_ms: 0,
            turn_total_ms: 100,
        }
    }

    #[test]
    fn delta_pct_handles_division_by_zero() {
        assert_eq!(delta_pct(10.0, 0.0), None);
        assert_eq!(delta_pct(10.0, 5.0), Some(100.0));
        assert_eq!(delta_pct(5.0, 10.0), Some(-50.0));
    }

    #[test]
    fn verdict_brackets_at_five_percent() {
        assert_eq!(verdict(Some(-4.99)), "wash");
        assert_eq!(verdict(Some(-5.01)), "ZH wins");
        assert_eq!(verdict(Some(5.01)),  "EN wins");
        assert_eq!(verdict(None),        "n/a");
    }

    #[test]
    fn summarize_run_computes_means_and_categories() {
        let started = Utc.with_ymd_and_hms(2026, 5, 26, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2026, 5, 26, 12, 1, 30).unwrap();
        let rows = vec![
            (rec("", 10, 8,  12, 200, 90000, 0,  18, 250, 90000, 0, false), "coding".to_string()),
            (rec("", 14, 12, 16, 240, 91000, 0,  22, 280, 91000, 0, false), "coding".to_string()),
            (rec("", 6,  5,  7,  100, 89000, 0,  10, 150, 89000, 0, false), "prose".to_string()),
        ];
        let s = summarize_run(
            "test-run".into(),
            started, finished,
            BackendKind::Api,
            "claude-sonnet-4-6".into(),
            "qwen3".into(),
            "bundled".into(),
            3, 0,
            &rows,
        );
        assert_eq!(s.n_attempted, 3);
        assert_eq!(s.n_succeeded, 3);
        assert_eq!(s.n_incomplete, 0);
        assert_eq!(s.n_failed, 0);
        assert!((s.mean_zh_input_reported - (12.0 + 16.0 + 7.0) / 3.0).abs() < 1e-9);
        assert!((s.mean_en_input_reported - (18.0 + 22.0 + 10.0) / 3.0).abs() < 1e-9);
        assert_eq!(s.per_category.len(), 2);
        assert_eq!(s.per_category["coding"].n, 2);
        assert_eq!(s.per_category["prose"].n, 1);
        assert_eq!(s.wall_ms, 90_000);
    }

    #[test]
    fn build_markdown_snapshot() {
        let started = Utc.with_ymd_and_hms(2026, 5, 26, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2026, 5, 26, 12, 1, 30).unwrap();
        let rows = vec![
            (rec("", 10, 8,  12, 200, 90000, 0,  18, 250, 90000, 0, false), "coding".to_string()),
            (rec("", 6,  5,  7,  100, 89000, 0,  10, 150, 89000, 0, false), "prose".to_string()),
        ];
        let summary = summarize_run(
            "2026-05-26T120000-test".into(),
            started, finished,
            BackendKind::ClaudeCode,
            "claude-sonnet-4-6".into(),
            "qwen3".into(),
            "bundled".into(),
            2, 0,
            &rows,
        );
        let report = RunReport { summary, rows };
        let md = build_markdown(&report);
        insta::assert_snapshot!(md);
    }

    #[test]
    fn build_csv_has_header_and_one_row_per_record() {
        let started = Utc.with_ymd_and_hms(2026, 5, 26, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2026, 5, 26, 12, 1, 30).unwrap();
        let rows = vec![
            (rec("", 10, 8,  12, 200, 90000, 0,  18, 250, 90000, 0, false), "coding".to_string()),
            (rec("", 6,  5,  7,  100, 89000, 0,  10, 150, 89000, 0, false), "prose".to_string()),
        ];
        let summary = summarize_run("rid".into(), started, finished, BackendKind::Api, "m".into(), "t".into(), "src".into(), 2, 0, &rows);
        let report = RunReport { summary, rows };
        let csv = build_csv(&report);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("run_id,prompt_index,category,prompt,"));
        assert!(lines[1].contains("rid,0,coding"));
        assert!(lines[2].contains("rid,1,prose"));
    }

    #[test]
    fn build_csv_header_and_data_rows_have_matching_column_counts() {
        let started = Utc.with_ymd_and_hms(2026, 5, 26, 12, 0, 0).unwrap();
        let finished = Utc.with_ymd_and_hms(2026, 5, 26, 12, 1, 30).unwrap();
        let rows = vec![
            (rec("", 10, 8, 12, 200, 90000, 0, 18, 250, 90000, 0, false), "coding".to_string()),
            (rec("", 6,  5,  7, 100, 89000, 0, 10, 150, 89000, 0, false), "prose".to_string()),
        ];
        let summary = summarize_run(
            "rid".into(), started, finished, BackendKind::Api,
            "m".into(), "t".into(), "src".into(), 2, 0, &rows,
        );
        let report = RunReport { summary, rows };
        let csv = build_csv(&report);
        let lines: Vec<&str> = csv.lines().collect();
        let header_cols = lines[0].split(',').count();
        for (i, line) in lines.iter().enumerate().skip(1) {
            // csv_quote may have introduced commas inside quotes — split naively still works for
            // these test inputs because the test rows don't contain commas in any field.
            let row_cols = line.split(',').count();
            assert_eq!(
                row_cols, header_cols,
                "row {} ({}) has {} columns but header has {}",
                i, line, row_cols, header_cols
            );
        }
    }

    #[test]
    fn csv_quote_escapes_commas_and_quotes() {
        assert_eq!(csv_quote("a,b"), "\"a,b\"");
        assert_eq!(csv_quote("she said \"hi\""), "\"she said \"\"hi\"\"\"");
        assert_eq!(csv_quote("plain"), "plain");
    }
}
