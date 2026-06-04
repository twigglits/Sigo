use sigo_core::TurnRecord;

pub struct Display {
    pub verbose: bool,
}

impl Display {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    pub fn print_turn_footer(&self, record: &TurnRecord) {
        let zh_in = fmt_opt(record.chinese_prompt_tokens_reported);
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
            "[turn {} · {} ms · ZH-in {} reported vs EN-proxy {} local]",
            record.turn_index, record.turn_total_ms, zh_in, record.english_prompt_tokens_local
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
