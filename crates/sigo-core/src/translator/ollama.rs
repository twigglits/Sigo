use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{prompts, Translator};
use crate::config::TranslatorStyle;
use crate::conversation::Direction;
use crate::error::{Result, SigoError};

#[derive(Debug, Clone)]
pub struct OllamaTranslator {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    timeout: Duration,
    en_to_zh_system: String,
    zh_to_en_system: String,
    options: GenOptions,
    keep_alive: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    stream: bool,
    options: &'a GenOptions,
    /// How long Ollama keeps the model resident after this call. Avoids a
    /// reload between turns (a common source of "transient" first-turn timeouts).
    keep_alive: &'a str,
}

/// Wrap a source text in the translate-not-answer markers (see prompts.rs).
fn wrap_source(text: &str) -> String {
    format!("<source>\n{text}\n</source>")
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    /// Owned because the final user turn wraps the source text in markers and
    /// few-shot user turns are wrapped the same way.
    content: String,
}

/// Generation options sent to Ollama. Defaults are tuned for a translation layer:
/// fully deterministic (`temperature 0` + fixed `seed`), a context window large
/// enough that typical prompts are not silently truncated, and an unbounded
/// `num_predict` so a long translation is never cut short.
#[derive(Debug, Clone, Serialize)]
struct GenOptions {
    temperature: f32,
    seed: u32,
    num_ctx: u32,
    num_predict: i32,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            seed: 0,
            num_ctx: 8192,
            num_predict: -1,
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

impl OllamaTranslator {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client builds");
        Self {
            client,
            endpoint: endpoint.into(),
            model: model.into(),
            timeout,
            en_to_zh_system: prompts::EN_TO_ZH_TERSE_SYSTEM.to_string(),
            zh_to_en_system: prompts::ZH_TO_EN_SYSTEM.to_string(),
            options: GenOptions::default(),
            keep_alive: "30m".to_string(),
        }
    }

    pub fn with_system_prompts(mut self, en_to_zh: String, zh_to_en: String) -> Self {
        self.en_to_zh_system = en_to_zh;
        self.zh_to_en_system = zh_to_en;
        self
    }

    /// Select the EN→ZH register. The ZH→EN prompt is style-independent: it
    /// produces the displayed answer and feeds the English control arm.
    pub fn with_style(mut self, style: TranslatorStyle) -> Self {
        self.en_to_zh_system = match style {
            TranslatorStyle::Terse => prompts::EN_TO_ZH_TERSE_SYSTEM,
            TranslatorStyle::Fluent => prompts::EN_TO_ZH_FLUENT_SYSTEM,
        }
        .to_string();
        self
    }

    /// Build the `/api/chat` request body for one translation: system prompt,
    /// translate-not-answer few-shot pairs, then the marker-wrapped source.
    /// Extracted so the protocol shape and the generation options (determinism,
    /// context window, keep-alive) are unit-testable without a live Ollama.
    fn build_body<'a>(&'a self, text: &str, dir: Direction) -> ChatRequest<'a> {
        let (system, few_shots) = match dir {
            Direction::EnToZh => (self.en_to_zh_system.as_str(), prompts::EN_TO_ZH_FEW_SHOTS),
            Direction::ZhToEn => (self.zh_to_en_system.as_str(), prompts::ZH_TO_EN_FEW_SHOTS),
        };
        let mut messages = Vec::with_capacity(2 + 2 * few_shots.len());
        messages.push(ChatMessage {
            role: "system",
            content: system.to_string(),
        });
        for (src, translation) in few_shots {
            messages.push(ChatMessage {
                role: "user",
                content: wrap_source(src),
            });
            messages.push(ChatMessage {
                role: "assistant",
                content: (*translation).to_string(),
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: wrap_source(text),
        });
        ChatRequest {
            model: &self.model,
            messages,
            stream: false,
            options: &self.options,
            keep_alive: &self.keep_alive,
        }
    }
}

#[async_trait]
impl Translator for OllamaTranslator {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
        // Structural code protection: fenced/inline code is replaced by
        // sentinels so the model cannot answer, alter, or drop it (observed
        // live in all three forms), then reinstated byte-for-byte.
        let masked = super::mask::mask_protected(text);
        let send_text = masked.as_ref().map_or(text, |m| m.text.as_str());
        let body = self.build_body(send_text, dir);
        let url = format!("{}/api/chat", self.endpoint.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SigoError::TranslatorTimeout(self.timeout)
                } else {
                    SigoError::Translator(e.to_string())
                }
            })?;
        if !resp.status().is_success() {
            return Err(SigoError::Translator(format!(
                "ollama status {}",
                resp.status()
            )));
        }
        let parsed: ChatResponse = resp.json().await?;
        match masked {
            Some(m) => super::mask::restore_protected(&parsed.message.content, &m.spans),
            None => Ok(parsed.message.content),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator() -> OllamaTranslator {
        OllamaTranslator::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(60),
        )
    }

    #[test]
    fn build_body_uses_source_protocol_with_few_shots() {
        // Translate-not-answer protocol: the source text travels between
        // <source> markers, preceded by few-shot pairs that demonstrate
        // translating instruction-shaped text instead of executing it (a live
        // qwen2.5:7b answered "Explain X" / "Write a limerick" prompts under
        // the naked-text protocol, silently replacing the user's question).
        let t = translator();
        let body = t.build_body("Explain X.", Direction::EnToZh);
        let shots = prompts::EN_TO_ZH_FEW_SHOTS;
        assert_eq!(body.messages.len(), 2 + 2 * shots.len());
        for (i, (src, out)) in shots.iter().enumerate() {
            let user = &body.messages[1 + 2 * i];
            let assistant = &body.messages[2 + 2 * i];
            assert_eq!(user.role, "user");
            assert!(user.content.contains("<source>"), "{}", user.content);
            assert!(user.content.contains(src), "{}", user.content);
            assert_eq!(assistant.role, "assistant");
            assert_eq!(assistant.content, *out);
        }
        let last = body.messages.last().unwrap();
        assert_eq!(last.role, "user");
        assert_eq!(last.content, "<source>\nExplain X.\n</source>");

        // Same protocol on the response side.
        let body = t.build_body("你好。", Direction::ZhToEn);
        assert_eq!(
            body.messages.len(),
            2 + 2 * prompts::ZH_TO_EN_FEW_SHOTS.len()
        );
        assert_eq!(
            body.messages.last().unwrap().content,
            "<source>\n你好。\n</source>"
        );
    }

    #[test]
    fn build_body_selects_system_prompt_by_style() {
        // Default register is terse (the token-minimizing product default).
        let t = translator();
        let body = t.build_body("hello", Direction::EnToZh);
        assert_eq!(body.messages[0].content, prompts::EN_TO_ZH_TERSE_SYSTEM);

        let t = translator().with_style(TranslatorStyle::Fluent);
        let body = t.build_body("hello", Direction::EnToZh);
        assert_eq!(body.messages[0].content, prompts::EN_TO_ZH_FLUENT_SYSTEM);

        // ZH->EN is style-independent.
        let t = translator().with_style(TranslatorStyle::Terse);
        let body = t.build_body("你好", Direction::ZhToEn);
        assert_eq!(body.messages[0].content, prompts::ZH_TO_EN_SYSTEM);
    }

    #[test]
    fn request_pins_determinism_and_context_options() {
        let t = translator();
        let body = t.build_body("hello", Direction::EnToZh);
        let json = serde_json::to_string(&body).unwrap();
        // temperature 0 alone is NOT reproducible; a fixed seed is required.
        assert!(json.contains("\"temperature\":0.0"), "body: {json}");
        assert!(json.contains("\"seed\""), "missing seed: {json}");
        // num_ctx guards against silent truncation of long EN->ZH prompts.
        assert!(json.contains("\"num_ctx\""), "missing num_ctx: {json}");
        // num_predict bounds pathological/runaway generations.
        assert!(
            json.contains("\"num_predict\""),
            "missing num_predict: {json}"
        );
        // keep_alive avoids model eviction between turns (a source of "transient" timeouts).
        assert!(
            json.contains("\"keep_alive\""),
            "missing keep_alive: {json}"
        );
    }
}

#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

    #[tokio::test]
    async fn code_blocks_survive_translation_byte_identical() {
        // Without masking, qwen2.5:7b SOLVED this refactor inside the fenced
        // block instead of translating the instruction around it.
        let t = OllamaTranslator::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(120),
        );
        let code = "```python\nresult = []\nfor x in items:\n    if x.active:\n        result.append(x.name.upper())\n```";
        let prompt = format!("Rewrite this loop to use a list comprehension:\n{code}");
        let zh = t.translate(&prompt, Direction::EnToZh).await.unwrap();
        assert!(
            zh.contains(code),
            "fenced block altered or dropped in translation: {zh}"
        );
        assert!(
            !zh.contains("[x.name.upper() for x in items"),
            "translator solved the task instead of translating it: {zh}"
        );
    }

    #[tokio::test]
    async fn instruction_prompts_are_translated_not_executed() {
        let t = OllamaTranslator::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(120),
        );
        let zh = t
            .translate(
                "Explain how Rust's borrow checker prevents data races, in under 200 words. \
                 Use one short code example.",
                Direction::EnToZh,
            )
            .await
            .unwrap();
        assert!(zh.contains("200"), "constraint lost: {zh}");
        assert!(
            !zh.contains("```"),
            "translator EXECUTED the task (emitted a code block): {zh}"
        );
        assert!(
            zh.chars().count() < 80,
            "translation suspiciously long — likely an answer, not a translation: {zh}"
        );
    }

    #[tokio::test]
    async fn terse_translation_preserves_numbers_and_identifiers() {
        let t = OllamaTranslator::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(120),
        );
        let zh = t
            .translate(
                "Do not change the public signature of parse_config in src/config.rs; \
                 all 12 existing tests must still pass.",
                Direction::EnToZh,
            )
            .await
            .unwrap();
        assert!(zh.contains("12"), "number lost in terse ZH: {zh}");
        assert!(zh.contains("parse_config"), "identifier lost: {zh}");
        assert!(zh.contains("src/config.rs"), "path lost: {zh}");
    }

    #[tokio::test]
    async fn roundtrip_against_local_ollama() {
        let t = OllamaTranslator::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(60),
        );
        let zh = t
            .translate("Hello, world!", Direction::EnToZh)
            .await
            .unwrap();
        assert!(!zh.is_empty());
        let en = t.translate(&zh, Direction::ZhToEn).await.unwrap();
        assert!(en.to_lowercase().contains("hello") || en.to_lowercase().contains("world"));
    }
}
