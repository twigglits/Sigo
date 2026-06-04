use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{prompts, Translator};
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
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    options: ChatOptions,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: f32,
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
            en_to_zh_system: prompts::EN_TO_ZH_SYSTEM.to_string(),
            zh_to_en_system: prompts::ZH_TO_EN_SYSTEM.to_string(),
        }
    }

    pub fn with_system_prompts(mut self, en_to_zh: String, zh_to_en: String) -> Self {
        self.en_to_zh_system = en_to_zh;
        self.zh_to_en_system = zh_to_en;
        self
    }
}

#[async_trait]
impl Translator for OllamaTranslator {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
        let system = match dir {
            Direction::EnToZh => self.en_to_zh_system.as_str(),
            Direction::ZhToEn => self.zh_to_en_system.as_str(),
        };
        let body = ChatRequest {
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
            options: ChatOptions { temperature: 0.0 },
        };
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

#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

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
