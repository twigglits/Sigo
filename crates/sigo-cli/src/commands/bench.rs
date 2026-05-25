use anyhow::Result;
use sigo_core::{read_jsonl, summarize, SigoConfig, TurnRecord};
use uuid::Uuid;

use crate::cli::BenchCommand;

pub async fn run(config: &SigoConfig, cmd: BenchCommand) -> Result<()> {
    let path = config.resolved_log_path();
    let records = read_jsonl(&path).map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;

    match cmd {
        BenchCommand::Summary { session, last } => {
            let filtered = filter(&records, session.as_deref(), last);
            let filtered_owned: Vec<TurnRecord> = filtered.iter().map(|r| (*r).clone()).collect();
            let s = summarize(&filtered_owned);
            println!("turns:                {}", s.turn_count);
            println!("sessions:             {}", s.session_count);
            println!("mean EN-prompt local:    {:.1}", s.mean_en_prompt_local);
            println!("mean ZH-prompt local:    {:.1}", s.mean_zh_prompt_local);
            if let Some(v) = s.mean_zh_prompt_reported {
                println!("mean ZH-prompt reported: {:.1}", v);
            }
            if let Some(v) = s.mean_zh_response_reported {
                println!("mean ZH-response reported: {:.1}", v);
            }
            if let Some(v) = s.calibration_factor {
                println!("calibration factor:   {:.3}", v);
            }
            if let Some(v) = s.estimated_savings_pct {
                println!("estimated savings:    {:+.1}% vs EN", v);
            }
            println!("cumulative ZH-prompt local: {}", s.cumulative_zh_prompt_local);
            println!("cumulative EN-prompt local: {}", s.cumulative_en_prompt_local);
        }
        BenchCommand::Show { session, turn } => {
            let sid: Uuid = session.parse().map_err(|e| anyhow::anyhow!("bad session uuid: {e}"))?;
            let found = records
                .iter()
                .find(|r| r.session_id == sid && r.turn_index == turn)
                .ok_or_else(|| anyhow::anyhow!("no record for session={sid} turn={turn}"))?;
            println!("{}", serde_json::to_string_pretty(found)?);
        }
        BenchCommand::Export { format, session } => {
            let filtered = filter(&records, session.as_deref(), None);
            match format.as_str() {
                "jsonl" => {
                    for r in &filtered {
                        println!("{}", serde_json::to_string(r)?);
                    }
                }
                "csv" => {
                    println!("session_id,turn_index,backend,en_prompt_local,zh_prompt_local,zh_prompt_reported,zh_response_reported,turn_total_ms");
                    for r in &filtered {
                        println!(
                            "{},{},{:?},{},{},{},{},{}",
                            r.session_id,
                            r.turn_index,
                            r.backend,
                            r.english_prompt_tokens_local,
                            r.chinese_prompt_tokens_local,
                            r.chinese_prompt_tokens_reported
                                .map(|v| v.to_string())
                                .unwrap_or_default(),
                            r.chinese_response_tokens_reported
                                .map(|v| v.to_string())
                                .unwrap_or_default(),
                            r.turn_total_ms,
                        );
                    }
                }
                other => anyhow::bail!("unknown format `{other}` (use `jsonl` or `csv`)"),
            }
        }
    }
    Ok(())
}

fn filter<'a>(
    records: &'a [TurnRecord],
    session: Option<&str>,
    last: Option<usize>,
) -> Vec<&'a TurnRecord> {
    let mut filtered: Vec<&TurnRecord> = match session {
        Some(s) => {
            let sid: Uuid = match s.parse() {
                Ok(u) => u,
                Err(_) => return vec![],
            };
            records.iter().filter(|r| r.session_id == sid).collect()
        }
        None => records.iter().collect(),
    };
    if let Some(n) = last {
        if filtered.len() > n {
            filtered = filtered.split_off(filtered.len() - n);
        }
    }
    filtered
}
