//! Offline terseness-compliance measurement over the bundled corpora, using the
//! exact production path: OllamaTranslator (terse + fluent registers) ->
//! compact_zh -> never-worse guard -> o200k proxy counts.
//! Run: cargo run -p sigo-core --example measure_pipeline > /tmp/pipeline_measure.csv
//! Requires a live Ollama with qwen2.5:7b at localhost:11434.
use sigo_core::compact::compact_zh;
use sigo_core::config::TranslatorStyle;
use sigo_core::conversation::Direction;
use sigo_core::tokenizer::{Tokenizer, TokenizerProxy};
use sigo_core::translator::{OllamaTranslator, Translator};
use std::time::Duration;

fn csv_escape(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

#[tokio::main]
async fn main() {
    let tk = TokenizerProxy::new().unwrap();
    let n = |s: &str| tk.count_tokens(s).unwrap();
    let terse = OllamaTranslator::new(
        "http://localhost:11434",
        "qwen2.5:7b",
        Duration::from_secs(300),
    )
    .with_style(TranslatorStyle::Terse);
    let fluent = OllamaTranslator::new(
        "http://localhost:11434",
        "qwen2.5:7b",
        Duration::from_secs(300),
    )
    .with_style(TranslatorStyle::Fluent);

    // Production guard: send compacted only if it does not count more.
    let sent = |raw: &str| -> (String, u32) {
        let c = compact_zh(raw);
        if n(&c) <= n(raw) {
            let t = n(&c);
            (c, t)
        } else {
            let t = n(raw);
            (raw.to_string(), t)
        }
    };

    let mut prompts: Vec<(String, String)> = sigo_core::load_default_corpus()
        .into_iter()
        .map(|e| (format!("chat/{}", e.category), e.prompt))
        .collect();
    prompts.extend(
        sigo_core::load_default_coding_corpus()
            .into_iter()
            .take(10)
            .map(|t| (format!("code/{}", t.task_id), t.prompt)),
    );

    println!(
        "category,en_tokens,fluent_raw,fluent_sent,terse_raw,terse_sent,en_prompt,terse_sent_text"
    );
    for (i, (cat, en)) in prompts.iter().enumerate() {
        let zh_t = match terse.translate(en, Direction::EnToZh).await {
            Ok(z) => z,
            Err(e) => {
                eprintln!("[{i}] terse translate failed: {e}");
                continue;
            }
        };
        let zh_f = match fluent.translate(en, Direction::EnToZh).await {
            Ok(z) => z,
            Err(e) => {
                eprintln!("[{i}] fluent translate failed: {e}");
                continue;
            }
        };
        let (zh_t_sent, t_sent) = sent(&zh_t);
        let (_zh_f_sent, f_sent) = sent(&zh_f);
        println!(
            "{cat},{},{},{},{},{},{},{}",
            n(en),
            n(&zh_f),
            f_sent,
            n(&zh_t),
            t_sent,
            csv_escape(en),
            csv_escape(&zh_t_sent)
        );
        eprintln!(
            "[{}/{}] {cat} en={} terse_sent={}",
            i + 1,
            prompts.len(),
            n(en),
            t_sent
        );
    }
}
