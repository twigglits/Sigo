use futures::stream::BoxStream;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_stream::wrappers::ReceiverStream;

use super::interactive::{self, InteractiveProc};
use super::question::{build_user_turn_line, QuestionRequest};
use super::{ClaudeBackend, ResponseChunk};
use crate::conversation::{Conversation, Usage};
use crate::error::{Result, SigoError};

/// Claude Code CLI backend.
///
/// Spawns the `claude` CLI process with `-p <prompt> --output-format stream-json`
/// and parses its NDJSON event stream. Supports session persistence across turns
/// (the session ID is captured from the `system` event and passed via `--resume`
/// on subsequent turns).
///
/// # Interactive mode
///
/// When a question channel is attached ([`with_question_channel`](Self::with_question_channel)
/// / [`attach_question_channel`](Self::attach_question_channel)), turns instead run on one
/// long-lived stream-json process and Claude Code's `AskUserQuestion` tool —
/// clarification questions with multiple-choice options — is forwarded to the
/// channel and answered mid-turn with the user's selections. Without a channel
/// the behavior is exactly the historic per-turn mode (questions auto-denied
/// by the CLI), which keeps `chat`, `bench run`, and eval arms unchanged.
///
/// # Performance
///
/// Each turn spawns a new process. The session resume mechanism mitigates cold-start
/// costs for multi-turn conversations by allowing the CLI to reuse cached context.
/// (Interactive mode keeps one process alive instead, preserving the prompt cache.)
#[derive(Debug, Clone)]
pub struct ClaudeCodeBackend {
    binary: String,
    extra_args: Vec<String>,
    model: Option<String>,
    session: Arc<AsyncMutex<Option<String>>>,
    question_tx: Option<mpsc::Sender<QuestionRequest>>,
    interactive: Arc<AsyncMutex<Option<InteractiveProc>>>,
}

impl ClaudeCodeBackend {
    /// Create a new Claude Code backend.
    ///
    /// `binary` is the path or name of the `claude` CLI executable on `PATH`.
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            extra_args: vec![],
            model: None,
            session: Arc::new(AsyncMutex::new(None)),
            question_tx: None,
            interactive: Arc::new(AsyncMutex::new(None)),
        }
    }

    /// Set extra CLI arguments passed to every `claude` invocation (e.g. `["--dangerously-skip-permissions"]`).
    pub fn with_extra_args(mut self, args: Vec<String>) -> Self {
        self.extra_args = args;
        self
    }

    /// Set the model override. When set, `--model <name>` is passed to the CLI.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Enable interactive mode: `AskUserQuestion` requests are forwarded to
    /// `tx` and answered mid-turn (see the type-level docs).
    pub fn with_question_channel(mut self, tx: mpsc::Sender<QuestionRequest>) -> Self {
        self.question_tx = Some(tx);
        self
    }

    /// Attach (or replace) the interactive question channel on an existing backend.
    pub fn attach_question_channel(&mut self, tx: mpsc::Sender<QuestionRequest>) {
        self.question_tx = Some(tx);
    }

    /// Forget the CLI session and kill any interactive process, so the next
    /// turn starts a brand-new conversation (REPL `/reset` & `/clear`).
    pub async fn reset_session(&self) {
        *self.session.lock().await = None;
        if let Some(proc) = self.interactive.lock().await.take() {
            proc.shutdown();
        }
    }

    /// Run one turn on the long-lived interactive process, (re)spawning it on
    /// first use, after process death, or when the first write fails because
    /// the process died between turns.
    async fn interactive_turn(
        &self,
        prompt: &str,
        qtx: mpsc::Sender<QuestionRequest>,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        let mut guard = self.interactive.lock().await;
        let user_line = build_user_turn_line(prompt);
        for attempt in 0..2 {
            if guard.as_ref().is_none_or(|p| !p.is_alive()) {
                *guard = Some(
                    interactive::spawn_interactive(
                        &self.binary,
                        self.model.as_deref(),
                        &self.extra_args,
                        self.session.clone(),
                        qtx.clone(),
                    )
                    .await?,
                );
            }
            let proc = guard.as_ref().expect("spawned above");
            let rx = proc.begin_turn()?;
            match proc.write_line(&user_line).await {
                Ok(()) => return Ok(Box::pin(ReceiverStream::new(rx))),
                Err(e) => {
                    // The process likely died between turns; retire it and
                    // retry once on a fresh spawn (resuming the session).
                    proc.abort_turn();
                    drop(rx);
                    if let Some(old) = guard.take() {
                        old.shutdown();
                    }
                    if attempt == 1 {
                        return Err(e);
                    }
                }
            }
        }
        unreachable!("loop returns on success or final error")
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

impl ClaudeBackend for ClaudeCodeBackend {
    async fn stream_turn(
        &self,
        _convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>> {
        if let Some(qtx) = &self.question_tx {
            return self.interactive_turn(prompt, qtx.clone()).await;
        }
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
        // Take stderr too: it must be drained concurrently (an unread pipe can fill
        // and deadlock the child) and is the only diagnostic when `claude` fails.
        let stderr = child.stderr.take();

        let session_handle = self.session.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ResponseChunk>>(64);

        tokio::spawn(async move {
            // Drain stderr in parallel with stdout so a chatty failure can't deadlock.
            let stderr_task = tokio::spawn(async move {
                let mut buf = String::new();
                if let Some(se) = stderr {
                    let _ = BufReader::new(se).read_to_string(&mut buf).await;
                }
                buf
            });

            let mut reader = BufReader::new(stdout).lines();
            let mut emitted_done = false;
            let mut sent_err = false;
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match parse_line(&line, &session_handle).await {
                            Ok(Some(chunk)) => {
                                if matches!(chunk, ResponseChunk::Done { .. }) {
                                    emitted_done = true;
                                }
                                if tx.send(Ok(chunk)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = tx.send(Err(e)).await;
                                sent_err = true;
                                break;
                            }
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => {
                        let _ = tx.send(Err(SigoError::from(e))).await;
                        sent_err = true;
                        break;
                    }
                }
            }
            // Reap the child and capture its exit status. A non-zero exit with no
            // `result` event (and no error already surfaced) would otherwise look
            // like a silent empty success — turn it into a real backend error.
            let status = child.wait().await;
            let stderr_text = stderr_task.await.unwrap_or_default();
            if !emitted_done && !sent_err {
                let (failed, code) = match &status {
                    Ok(s) => (!s.success(), s.code()),
                    Err(_) => (true, None),
                };
                if failed {
                    let code = code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let detail = stderr_text.trim();
                    let msg = if detail.is_empty() {
                        format!("claude exited with status {code} and produced no result")
                    } else {
                        format!("claude exited with status {code}: {detail}")
                    };
                    let _ = tx.send(Err(SigoError::Backend(msg))).await;
                }
            }
            // tx drops here, completing the consumer stream.
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

pub(crate) async fn parse_line(
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

    #[cfg(unix)]
    fn write_exec(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(unix)]
    async fn collect(backend: &ClaudeCodeBackend) -> (Vec<String>, bool) {
        use futures::StreamExt;
        let mut stream = backend
            .stream_turn(&Conversation::new(), "hi")
            .await
            .unwrap();
        let mut texts = Vec::new();
        let mut got_err = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(ResponseChunk::TextDelta(t)) => texts.push(t),
                Ok(ResponseChunk::Done { .. }) => {}
                Err(_) => got_err = true,
            }
        }
        (texts, got_err)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_exit_without_result_surfaces_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude");
        // No stdout JSON at all, writes to stderr, exits non-zero.
        write_exec(
            &script,
            "#!/bin/sh\necho 'boom: model unavailable' >&2\nexit 1\n",
        );
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap().to_string());
        let (texts, got_err) = collect(&backend).await;
        assert!(texts.is_empty());
        assert!(
            got_err,
            "a non-zero claude exit with no result event must surface an error, not a silent empty success"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clean_result_event_yields_done_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude");
        write_exec(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}'\nprintf '%s\\n' '{\"type\":\"result\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\nexit 0\n",
        );
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap().to_string());
        let (texts, got_err) = collect(&backend).await;
        assert_eq!(texts, vec!["hi".to_string()]);
        assert!(!got_err, "a clean exit with a result event must not error");
    }
}
