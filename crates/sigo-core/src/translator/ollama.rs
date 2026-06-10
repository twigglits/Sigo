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
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    options: &'a GenOptions,
    /// How long Ollama keeps the model resident after this call. Avoids a
    /// reload between turns (a common source of "transient" first-turn timeouts).
    keep_alive: &'a str,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
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

    /// Build the `/api/chat` request body for one translation. Extracted so the
    /// generation options (determinism, context window, keep-alive) are unit-testable
    /// without a live Ollama.
    fn build_body<'a>(&'a self, text: &'a str, dir: Direction) -> ChatRequest<'a> {
        let system = match dir {
            Direction::EnToZh => self.en_to_zh_system.as_str(),
            Direction::ZhToEn => self.zh_to_en_system.as_str(),
        };
        ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: text,
                },
            ],
            stream: false,
            options: &self.options,
            keep_alive: &self.keep_alive,
        }
    }
}

#[async_trait]
impl Translator for OllamaTranslator {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
        let body = self.build_body(text, dir);
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
        Ok(parsed.message.content)
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
