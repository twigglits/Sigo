use serde::Serialize;
use std::path::Path;

use super::TurnRecord;
use crate::error::Result;

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub turn_count: usize,
    pub session_count: usize,
    pub mean_en_prompt_local: f64,
    pub mean_zh_prompt_local: f64,
    pub mean_zh_prompt_reported: Option<f64>,
    pub mean_zh_response_reported: Option<f64>,
    pub calibration_factor: Option<f64>,
    pub estimated_savings_pct: Option<f64>,
    pub cumulative_zh_prompt_local: u32,
    pub cumulative_en_prompt_local: u32,
}

pub fn read_jsonl(path: &Path) -> Result<Vec<TurnRecord>> {
    let s = std::fs::read_to_string(path)?;
    let mut out = vec![];
    for (i, line) in s.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let r: TurnRecord = serde_json::from_str(line).map_err(|e| {
            crate::error::SigoError::Sink(format!("line {}: {e}", i + 1))
        })?;
        out.push(r);
    }
    Ok(out)
}

pub fn summarize(records: &[TurnRecord]) -> Summary {
    if records.is_empty() {
        return Summary::default();
    }
    let n = records.len() as f64;

    let mean_en_prompt_local =
        records.iter().map(|r| r.english_prompt_tokens_local as f64).sum::<f64>() / n;
    let mean_zh_prompt_local =
        records.iter().map(|r| r.chinese_prompt_tokens_local as f64).sum::<f64>() / n;

    let reported_pairs: Vec<(u32, u32)> = records
        .iter()
        .filter_map(|r| {
            r.chinese_prompt_tokens_reported
                .map(|rep| (r.chinese_prompt_tokens_local, rep))
        })
        .collect();

    let calibration_factor = if !reported_pairs.is_empty() {
        let ratios: Vec<f64> = reported_pairs
            .iter()
            .filter(|(local, _)| *local > 0)
            .map(|(local, rep)| *rep as f64 / *local as f64)
            .collect();
        if ratios.is_empty() {
            None
        } else {
            Some(ratios.iter().sum::<f64>() / ratios.len() as f64)
        }
    } else {
        None
    };

    let mean_zh_prompt_reported = if !reported_pairs.is_empty() {
        Some(reported_pairs.iter().map(|(_, r)| *r as f64).sum::<f64>() / reported_pairs.len() as f64)
    } else {
        None
    };

    let response_reported: Vec<u32> = records
        .iter()
        .filter_map(|r| r.chinese_response_tokens_reported)
        .collect();
    let mean_zh_response_reported = if !response_reported.is_empty() {
        Some(response_reported.iter().sum::<u32>() as f64 / response_reported.len() as f64)
    } else {
        None
    };

    let estimated_savings_pct = match (mean_zh_prompt_reported, calibration_factor) {
        (Some(zh_rep), Some(cal)) if mean_en_prompt_local > 0.0 => {
            let en_estimated = mean_en_prompt_local * cal;
            if en_estimated > 0.0 {
                Some((zh_rep - en_estimated) / en_estimated * 100.0)
            } else {
                None
            }
        }
        _ => None,
    };

    let cumulative_zh_prompt_local = records
        .last()
        .map(|r| r.chinese_cumulative_prompt_tokens_local)
        .unwrap_or(0);
    let cumulative_en_prompt_local = records
        .last()
        .map(|r| r.english_cumulative_prompt_tokens_local)
        .unwrap_or(0);

    let session_count = {
        let mut ids: Vec<_> = records.iter().map(|r| r.session_id).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    };

    Summary {
        turn_count: records.len(),
        session_count,
        mean_en_prompt_local,
        mean_zh_prompt_local,
        mean_zh_prompt_reported,
        mean_zh_response_reported,
        calibration_factor,
        estimated_savings_pct,
        cumulative_zh_prompt_local,
        cumulative_en_prompt_local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::BackendKind;
    use chrono::Utc;
    use uuid::Uuid;

    fn rec(en_local: u32, zh_local: u32, zh_reported: Option<u32>) -> TurnRecord {
        TurnRecord {
            schema_version: 1,
            session_id: Uuid::nil(),
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
            chinese_prompt_tokens_reported: zh_reported,
            chinese_response_tokens_reported: None,
            cache_read_tokens_reported: None,
            cache_write_tokens_reported: None,
            chinese_cumulative_prompt_tokens_local: zh_local,
            english_cumulative_prompt_tokens_local: en_local,
            english_control_run: None,
            incomplete: false,
            turn_errors: vec![],
            translation_in_ms: 0,
            translation_out_ms_total: 0,
            translation_out_calls: 0,
            claude_ttft_ms: 0,
            claude_total_ms: 0,
            turn_total_ms: 0,
        }
    }

    #[test]
    fn summary_empty() {
        let s = summarize(&[]);
        assert_eq!(s.turn_count, 0);
    }

    #[test]
    fn summary_basic_calibration_and_savings() {
        // EN local 10, ZH local 8, ZH reported 12 → calibration 12/8 = 1.5;
        // estimated EN reported = 10*1.5 = 15; savings = (12-15)/15 = -20%
        let recs = vec![rec(10, 8, Some(12))];
        let s = summarize(&recs);
        assert!((s.calibration_factor.unwrap() - 1.5).abs() < 1e-9);
        let savings = s.estimated_savings_pct.unwrap();
        assert!((savings - (-20.0)).abs() < 1e-6, "got {savings}");
    }
}
