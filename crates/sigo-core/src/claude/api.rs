use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{stream::BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Role, Usage};
use crate::error::{Result, SigoError};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Cap on connection establishment. A dead endpoint should fail fast, not hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-read stall guard. Resets after each received byte, so it bounds a *stalled*
/// SSE socket without cutting a legitimately slow stream (Anthropic emits periodic
/// `ping` events that keep this well-fed during long generations).
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Total attempts (initial + retries) for the pre-stream `send()`.
const MAX_ATTEMPTS: u32 = 3;

/// Parse a `Retry-After` header expressed as integer seconds (the form Anthropic sends).
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?;
    let secs = v.to_str().ok()?.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(secs))
}

/// Whether a response status is worth retrying the (not-yet-streaming) request.
/// 401/403 are handled separately as auth failures and never reach here.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    use reqwest::StatusCode;
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error() // covers 500/502/503/504 and Anthropic's 529 overloaded
}

/// Exponential backoff: 250ms, 500ms, 1s, … capped at 8s.
fn backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(5);
    Duration::from_millis((250u64.saturating_mul(1u64 << shift)).min(8000))
}

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
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .expect("reqwest client builds");
        Self {
            client,
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
    MessageStart {
        message: MessageStartPayload,
    },
    ContentBlockDelta {
        delta: ContentDelta,
    },
    MessageDelta {
        delta: MessageDeltaPayload,
        usage: MessageDeltaUsage,
    },
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
    TextDelta {
        text: String,
    },
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
        let mut messages: Vec<RequestMessage> = convo
            .messages
            .iter()
            .map(|m| RequestMessage {
                role: match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                content: &m.content,
            })
            .collect();
        messages.push(RequestMessage {
            role: "user",
            content: prompt,
        });

        let body = MessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            stream: true,
            system: convo.system.as_deref(),
            messages,
        };

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        // Retry only the pre-stream `send()`: connect/timeout failures and transient
        // statuses (429 + 5xx) are safe to retry because no response bytes have been
        // emitted yet. Once the SSE body starts, we never retry (would duplicate output).
        let mut attempt: u32 = 0;
        let resp = loop {
            attempt += 1;
            let send_result = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await;

            match send_result {
                Ok(resp) => {
                    let status = resp.status();
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        let body = resp.text().await.unwrap_or_default();
                        return Err(SigoError::Auth(body));
                    }
                    if status.is_success() {
                        break resp;
                    }
                    if is_retryable_status(status) && attempt < MAX_ATTEMPTS {
                        let wait = parse_retry_after(resp.headers())
                            .unwrap_or_else(|| backoff_delay(attempt));
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        return Err(SigoError::RateLimited {
                            retry_after: parse_retry_after(resp.headers()),
                        });
                    }
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SigoError::Backend(format!("status {status}: {body}")));
                }
                Err(e) => {
                    if (e.is_connect() || e.is_timeout()) && attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(backoff_delay(attempt)).await;
                        continue;
                    }
                    return Err(e.into());
                }
            }
        };

        let event_stream = resp.bytes_stream().eventsource();
        let mut accumulated = AccumulatedUsage::default();
        let chunks = event_stream
            .map(move |event| -> Result<Option<ResponseChunk>> {
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
                    StreamEvent::ContentBlockDelta {
                        delta: ContentDelta::TextDelta { text },
                    } => Ok(Some(ResponseChunk::TextDelta(text))),
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
            })
            .filter_map(|r| async move {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn parse_retry_after_reads_integer_seconds() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "30".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(30)));
    }

    #[test]
    fn parse_retry_after_absent_or_garbage_is_none() {
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("retry-after", "soon".parse().unwrap());
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn retryable_statuses_cover_429_and_5xx_only() {
        use reqwest::StatusCode;
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_status(StatusCode::from_u16(529).unwrap())); // Anthropic overloaded
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert!(backoff_delay(2) > backoff_delay(1));
        assert!(backoff_delay(3) > backoff_delay(2));
        assert!(backoff_delay(50) <= Duration::from_millis(8000));
    }

    /// A throwaway TCP server that replies with a fixed raw HTTP response to every
    /// connection and counts how many connections it accepted.
    async fn spawn_status_server(response: &'static str) -> (String, std::sync::Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                c.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await; // drain the request so the client's send completes
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), count)
    }

    #[tokio::test]
    async fn retries_server_errors_up_to_max_attempts() {
        let resp =
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (url, count) = spawn_status_server(resp).await;
        let backend = ApiBackend::new("sk-test", "claude-sonnet-4-6", 64).with_base_url(url);
        let res = backend.stream_turn(&Conversation::new(), "hi").await;
        assert!(res.is_err(), "a 503 should surface an error");
        assert_eq!(
            count.load(Ordering::SeqCst),
            MAX_ATTEMPTS as usize,
            "send() should be retried up to MAX_ATTEMPTS on 5xx"
        );
    }

    #[tokio::test]
    async fn rate_limit_surfaces_parsed_retry_after() {
        let resp =
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (url, count) = spawn_status_server(resp).await;
        let backend = ApiBackend::new("sk-test", "m", 64).with_base_url(url);
        match backend.stream_turn(&Conversation::new(), "hi").await {
            Err(SigoError::RateLimited { retry_after }) => {
                assert_eq!(retry_after, Some(Duration::from_secs(0)))
            }
            Err(e) => panic!("expected RateLimited with a parsed Retry-After, got error: {e}"),
            Ok(_) => panic!("expected RateLimited, got an Ok stream"),
        }
        assert_eq!(count.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
    }
}
