use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{stream::BoxStream, StreamExt};
use serde::{Deserialize, Serialize};

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Role, Usage};
use crate::error::{Result, SigoError};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub struct ApiBackend {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
}

impl ApiBackend {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: ANTHROPIC_API_BASE.to_string(),
            model: model.into(),
            max_tokens,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<RequestMessage<'a>>,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart { message: MessageStartPayload },
    ContentBlockDelta { delta: ContentDelta },
    MessageDelta { delta: MessageDeltaPayload, usage: MessageDeltaUsage },
    MessageStop,
    Ping,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
struct MessageStartPayload {
    usage: MessageStartUsage,
}

#[derive(Deserialize, Debug)]
struct MessageStartUsage {
    input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentDelta {
    TextDelta { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
struct MessageDeltaPayload {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct MessageDeltaUsage {
    output_tokens: u32,
}

#[derive(Default)]
struct AccumulatedUsage {
    input_tokens: u32,
    output_tokens: u32,
    cache_read: Option<u32>,
    cache_write: Option<u32>,
    stop_reason: Option<String>,
}

#[async_trait]
impl ClaudeBackend for ApiBackend {
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        let mut messages: Vec<RequestMessage> = convo.messages.iter().map(|m| RequestMessage {
            role: match m.role { Role::User => "user", Role::Assistant => "assistant" },
            content: &m.content,
        }).collect();
        messages.push(RequestMessage { role: "user", content: prompt });

        let body = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            stream: true,
            system: convo.system.as_deref(),
            messages,
        };

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self.client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let body = resp.text().await.unwrap_or_default();
            return Err(SigoError::Auth(body));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SigoError::RateLimited { retry_after: None });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SigoError::Backend(format!("status {status}: {body}")));
        }

        let event_stream = resp.bytes_stream().eventsource();
        let mut accumulated = AccumulatedUsage::default();
        let chunks = event_stream.map(move |event| -> Result<Option<ResponseChunk>> {
            let event = event.map_err(|e| SigoError::Backend(format!("sse: {e}")))?;
            if event.data.is_empty() {
                return Ok(None);
            }
            let parsed: StreamEvent = serde_json::from_str(&event.data)?;
            match parsed {
                StreamEvent::MessageStart { message } => {
                    accumulated.input_tokens = message.usage.input_tokens;
                    accumulated.cache_read = message.usage.cache_read_input_tokens;
                    accumulated.cache_write = message.usage.cache_creation_input_tokens;
                    Ok(None)
                }
                StreamEvent::ContentBlockDelta { delta: ContentDelta::TextDelta { text } } => {
                    Ok(Some(ResponseChunk::TextDelta(text)))
                }
                StreamEvent::MessageDelta { delta, usage } => {
                    accumulated.output_tokens = usage.output_tokens;
                    accumulated.stop_reason = delta.stop_reason;
                    Ok(None)
                }
                StreamEvent::MessageStop => {
                    let u = Usage {
                        input_tokens: accumulated.input_tokens,
                        output_tokens: accumulated.output_tokens,
                        cache_read: accumulated.cache_read,
                        cache_write: accumulated.cache_write,
                    };
                    Ok(Some(ResponseChunk::Done {
                        usage: u,
                        stop_reason: accumulated.stop_reason.clone(),
                    }))
                }
                _ => Ok(None),
            }
        }).filter_map(|r| async move {
            match r {
                Ok(Some(c)) => Some(Ok(c)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            }
        });

        Ok(Box::pin(chunks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn constructor_stores_fields() {
        let b = ApiBackend::new("sk-test", "claude-sonnet-4-6", 4096);
        assert_eq!(b.model, "claude-sonnet-4-6");
        assert_eq!(b.max_tokens, 4096);
        assert_eq!(b.api_key, "sk-test");
        assert_eq!(b.base_url, ANTHROPIC_API_BASE);
    }

    #[tokio::test]
    async fn with_base_url_overrides_default() {
        let b = ApiBackend::new("sk-test", "claude-sonnet-4-6", 4096)
            .with_base_url("http://localhost:9999");
        assert_eq!(b.base_url, "http://localhost:9999");
    }
}
