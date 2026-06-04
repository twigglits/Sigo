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
    let back = translator.translate(zh_prompt, Direction::ZhToEn).await.ok()?;
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
struct ChatRequest<'a> { model: &'a str, messages: Vec<Msg<'a>>, stream: bool }
#[derive(Serialize)]
struct Msg<'a> { role: &'a str, content: &'a str }
#[derive(Deserialize)]
struct ChatResponse { message: RespMsg }
#[derive(Deserialize)]
struct RespMsg { content: String }

impl OllamaJudge {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, timeout: Duration) -> Self {
        let client = reqwest::Client::builder().timeout(timeout).build().expect("reqwest builds");
        Self { client, endpoint: endpoint.into(), model: model.into() }
    }
}

#[async_trait]
impl Judge for OllamaJudge {
    async fn score(&self, original: &str, candidate: &str) -> Result<u8> {
        let user = format!("ORIGINAL:\n{original}\n\nCANDIDATE:\n{candidate}");
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Msg { role: "system", content: RUBRIC },
                Msg { role: "user", content: &user },
            ],
            stream: false,
        };
        let url = format!("{}/api/chat", self.endpoint.trim_end_matches('/'));
        let resp = self.client.post(&url).json(&body).send().await
            .map_err(|e| SigoError::Eval(format!("judge request: {e}")))?;
        if !resp.status().is_success() {
            return Err(SigoError::Eval(format!("judge status {}", resp.status())));
        }
        let parsed: ChatResponse = resp.json().await.map_err(|e| SigoError::Eval(e.to_string()))?;
        parse_score(&parsed.message.content)
            .ok_or_else(|| SigoError::Eval(format!("unparseable judge reply: {:?}", parsed.message.content)))
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
}
