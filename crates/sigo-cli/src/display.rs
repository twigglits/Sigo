use sigo_core::TurnRecord;

pub struct Display {
    pub verbose: bool,
}

impl Display {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    pub fn print_turn_footer(&self, record: &TurnRecord) {
        let savings = estimate_savings_pct(record)
            .map(|p| format!("{:+.0}%", p))
            .unwrap_or_else(|| "n/a".to_string());
        println!();
        if self.verbose {
            println!("─── ZH prompt ──────────────────");
            println!("{}", record.chinese_prompt);
            println!("─── ZH response ────────────────");
            println!("{}", record.chinese_response);
            println!("─── tokens ─────────────────────");
            println!("  EN-prompt local:   {}", record.english_prompt_tokens_local);
            println!(
                "  ZH-prompt local:   {}  reported: {}",
                record.chinese_prompt_tokens_local,
                fmt_opt(record.chinese_prompt_tokens_reported),
            );
            println!(
                "  ZH-response local: {}  reported: {}",
                record.chinese_response_tokens_local,
                fmt_opt(record.chinese_response_tokens_reported),
            );
            if let Some(ctrl) = &record.english_control_run {
                println!(
                    "  EN-control prompt: {}  response: {}",
                    ctrl.prompt_tokens_reported, ctrl.response_tokens_reported
                );
            }
            println!("─── timing ─────────────────────");
            println!(
                "  EN→ZH: {} ms  ZH-stream: {} ms (ttft {})  ZH→EN total: {} ms × {} calls  total {} ms",
                record.translation_in_ms,
                record.claude_total_ms,
                record.claude_ttft_ms,
                record.translation_out_ms_total,
                record.translation_out_calls,
                record.turn_total_ms,
            );
        }
        println!(
            "[turn {} · {} ms · {} vs EN local-est]",
            record.turn_index, record.turn_total_ms, savings
        );
        if !record.turn_errors.is_empty() {
            println!("(turn-errors: {})", record.turn_errors.join("; "));
        }
        if record.incomplete {
            println!("(turn incomplete — conversation history did not advance)");
        }
    }
}

fn fmt_opt(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string())
}

fn estimate_savings_pct(record: &TurnRecord) -> Option<f64> {
    if record.english_prompt_tokens_local == 0 {
        return None;
    }
    let calibration = match (record.chinese_prompt_tokens_local, record.chinese_prompt_tokens_reported) {
        (0, _) | (_, None) => 1.0,
        (l, Some(r)) if l > 0 => r as f64 / l as f64,
        _ => 1.0,
    };
    let estimated_en_reported = record.english_prompt_tokens_local as f64 * calibration;
    let actual_zh_reported = record
        .chinese_prompt_tokens_reported
        .unwrap_or(record.chinese_prompt_tokens_local) as f64;
    if estimated_en_reported == 0.0 {
        return None;
    }
    Some((actual_zh_reported - estimated_en_reported) / estimated_en_reported * 100.0)
}
