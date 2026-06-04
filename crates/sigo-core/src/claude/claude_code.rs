use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;
use tokio_stream::wrappers::ReceiverStream;

use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Usage};
use crate::error::{Result, SigoError};

#[derive(Debug, Clone)]
pub struct ClaudeCodeBackend {
    binary: String,
    extra_args: Vec<String>,
    model: Option<String>,
    session: Arc<AsyncMutex<Option<String>>>,
}

impl ClaudeCodeBackend {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            extra_args: vec![],
            model: None,
            session: Arc::new(AsyncMutex::new(None)),
        }
    }

    pub fn with_extra_args(mut self, args: Vec<String>) -> Self {
        self.extra_args = args;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CcEvent {
    System {
        session_id: Option<String>,
    },
    Assistant {
        message: CcAssistantMessage,
    },
    Result {
        #[serde(default)]
        usage: Option<CcUsage>,
        #[serde(default)]
        session_id: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
struct CcAssistantMessage {
    content: Vec<CcContent>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CcContent {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
struct CcUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

#[async_trait]
impl ClaudeBackend for ClaudeCodeBackend {
    async fn stream_turn(
        &self,
        _convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        let resume_session = self.session.lock().await.clone();

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("stream-json");
        cmd.arg("--verbose");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        if let Some(session) = &resume_session {
            cmd.arg("--resume").arg(session);
        }
        for a in &self.extra_args {
            cmd.arg(a);
        }
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| SigoError::Backend(format!("spawn {}: {e}", self.binary)))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SigoError::Backend("no stdout from child".into()))?;

        let session_handle = self.session.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ResponseChunk>>(64);

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match parse_line(&line, &session_handle).await {
                            Ok(Some(chunk)) => {
                                if tx.send(Ok(chunk)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = tx.send(Err(e)).await;
                                break;
                            }
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => {
                        let _ = tx.send(Err(SigoError::from(e))).await;
                        break;
                    }
                }
            }
            // Wait for the child to finish so it's reaped cleanly.
            let _ = child.wait().await;
            // tx drops here, completing the consumer stream.
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

async fn parse_line(
    line: &str,
    session_handle: &Arc<AsyncMutex<Option<String>>>,
) -> Result<Option<ResponseChunk>> {
    let event: CcEvent = serde_json::from_str(line)?;
    match event {
        CcEvent::System { session_id } => {
            if let Some(sid) = session_id {
                *session_handle.lock().await = Some(sid);
            }
            Ok(None)
        }
        CcEvent::Assistant { message } => {
            let mut combined = String::new();
            for c in message.content {
                if let CcContent::Text { text } = c {
                    combined.push_str(&text);
                }
            }
            if combined.is_empty() {
                Ok(None)
            } else {
                Ok(Some(ResponseChunk::TextDelta(combined)))
            }
        }
        CcEvent::Result { usage, session_id } => {
            if let Some(sid) = session_id {
                *session_handle.lock().await = Some(sid);
            }
            let u = usage
                .map(|u| Usage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    cache_read: u.cache_read_input_tokens,
                    cache_write: u.cache_creation_input_tokens,
                })
                .unwrap_or_default();
            Ok(Some(ResponseChunk::Done {
                usage: u,
                stop_reason: Some("end_turn".to_string()),
            }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_text_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"你好"}]}}"#;
        let parsed: CcEvent = serde_json::from_str(line).unwrap();
        match parsed {
            CcEvent::Assistant { message } => {
                assert_eq!(message.content.len(), 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_system_session_id_event() {
        let line = r#"{"type":"system","session_id":"abc-123"}"#;
        let parsed: CcEvent = serde_json::from_str(line).unwrap();
        match parsed {
            CcEvent::System { session_id } => assert_eq!(session_id.as_deref(), Some("abc-123")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_result_usage_event() {
        let line = r#"{"type":"result","usage":{"input_tokens":10,"output_tokens":20}}"#;
        let parsed: CcEvent = serde_json::from_str(line).unwrap();
        match parsed {
            CcEvent::Result { usage, .. } => {
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 20);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_result_without_usage() {
        let line = r#"{"type":"result"}"#;
        let parsed: CcEvent = serde_json::from_str(line).unwrap();
        match parsed {
            CcEvent::Result { usage, session_id } => {
                assert!(usage.is_none());
                assert!(session_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn parses_other_event_type_gracefully() {
        let line = r#"{"type":"unknown_future_event","data":42}"#;
        let parsed: CcEvent = serde_json::from_str(line).unwrap();
        assert!(matches!(parsed, CcEvent::Other));
    }
}
