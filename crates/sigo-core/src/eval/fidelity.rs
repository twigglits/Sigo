use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::conversation::Direction;
use crate::error::{Result, SigoError};
use crate::translator::Translator;

/// Scores semantic closeness of two English strings on 0..=10.
#[async_trait]
pub trait Judge: Send + Sync {
    async fn score(&self, original: &str, candidate: &str) -> Result<u8>;
}

/// Parse the first integer 0..=10 out of a judge's free-text reply.
pub fn parse_score(s: &str) -> Option<u8> {
    let mut digits = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse::<u8>().ok().filter(|n| *n <= 10)
}

/// Round-trip prompt fidelity: back-translate `zh_prompt` to English, then judge
/// closeness to `original_en`. Returns `None` on any failure (diagnostic only).
pub async fn roundtrip_fidelity(
    translator: &dyn Translator,
    judge: &dyn Judge,
    original_en: &str,
    zh_prompt: &str,
) -> Option<u8> {
    let back = translator
        .translate(zh_prompt, Direction::ZhToEn)
        .await
        .ok()?;
    judge.score(original_en, &back).await.ok()
}

const RUBRIC: &str = "You compare two English texts: ORIGINAL and CANDIDATE. \
Reply with ONLY a single integer 0-10 for how completely CANDIDATE preserves the \
meaning and intent of ORIGINAL (10 = identical meaning, 0 = unrelated).";

/// Ollama-backed judge, mirroring `OllamaTranslator`'s `/api/chat` shape.
pub struct OllamaJudge {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
    stream: bool,
    options: JudgeOptions,
    keep_alive: &'a str,
}

/// The judge's entire contract is "reply with ONLY a single integer", so it must
/// be deterministic: `temperature 0` + a fixed `seed`. `keep_alive` mirrors the
/// translator so the shared model is not evicted between calls.
#[derive(Serialize)]
struct JudgeOptions {
    temperature: f32,
    seed: u32,
}

impl Default for JudgeOptions {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            seed: 0,
        }
    }
}
#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}
#[derive(Deserialize)]
struct ChatResponse {
    message: RespMsg,
}
#[derive(Deserialize)]
struct RespMsg {
    content: String,
}

impl OllamaJudge {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest builds");
        Self {
            client,
            endpoint: endpoint.into(),
            model: model.into(),
        }
    }

    /// Build the `/api/chat` body for a judge call. Extracted so the determinism
    /// options are unit-testable without a live Ollama.
    fn build_body<'a>(&'a self, user: &'a str) -> ChatRequest<'a> {
        ChatRequest {
            model: &self.model,
            messages: vec![
                Msg {
                    role: "system",
                    content: RUBRIC,
                },
                Msg {
                    role: "user",
                    content: user,
                },
            ],
            stream: false,
            options: JudgeOptions::default(),
            keep_alive: "30m",
        }
    }
}

#[async_trait]
impl Judge for OllamaJudge {
    async fn score(&self, original: &str, candidate: &str) -> Result<u8> {
        let user = format!("ORIGINAL:\n{original}\n\nCANDIDATE:\n{candidate}");
        let body = self.build_body(&user);
        let url = format!("{}/api/chat", self.endpoint.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SigoError::Eval(format!("judge request: {e}")))?;
        if !resp.status().is_success() {
            return Err(SigoError::Eval(format!("judge status {}", resp.status())));
        }
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| SigoError::Eval(e.to_string()))?;
        parse_score(&parsed.message.content).ok_or_else(|| {
            SigoError::Eval(format!(
                "unparseable judge reply: {:?}",
                parsed.message.content
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_various_score_shapes() {
        assert_eq!(parse_score("8"), Some(8));
        assert_eq!(parse_score("Score: 7/10"), Some(7));
        assert_eq!(parse_score("I'd say 10."), Some(10));
        assert_eq!(parse_score("11"), None);
        assert_eq!(parse_score("no number"), None);
    }

    #[test]
    fn judge_request_is_deterministic_and_keeps_model_warm() {
        let j = OllamaJudge::new(
            "http://localhost:11434",
            "qwen2.5:7b",
            Duration::from_secs(60),
        );
        let json = serde_json::to_string(&j.build_body("ORIGINAL:\nx\n\nCANDIDATE:\ny")).unwrap();
        assert!(json.contains("\"temperature\":0.0"), "body: {json}");
        assert!(json.contains("\"seed\""), "missing seed: {json}");
        assert!(
            json.contains("\"keep_alive\""),
            "missing keep_alive: {json}"
        );
    }
}
