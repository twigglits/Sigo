//! Long-lived interactive `claude` process for AskUserQuestion passthrough.
//!
//! When a question channel is attached to [`ClaudeCodeBackend`](super::ClaudeCodeBackend),
//! turns run on ONE long-lived `claude` process per session:
//!
//! ```text
//! claude -p --input-format stream-json --output-format stream-json \
//!        --verbose --permission-prompt-tool stdio [--model M] [--resume SID]
//! ```
//!
//! User turns are written to the process's stdin as NDJSON `user` lines; each
//! turn's events stream back until a `result` line. `--permission-prompt-tool
//! stdio` routes tool permission decisions to this channel as
//! `control_request` lines: `AskUserQuestion` requests are forwarded to the
//! attached handler (the REPL's picker) and answered mid-turn with the user's
//! selections; every other tool is denied, preserving the CLI's own headless
//! auto-deny semantics. This is the only mode in which a pending question can
//! actually be answered — a per-turn `--resume` respawn dies with the
//! pending request (verified live on claude 2.1.173).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use super::claude_code::parse_line;
use super::question::{
    build_allow_line, build_deny_line, parse_control_line, parse_questions, CanUseTool,
    ControlEvent, QuestionReply, QuestionRequest,
};
use super::ResponseChunk;
use crate::error::{Result, SigoError};

pub(crate) type TurnSender = mpsc::Sender<Result<ResponseChunk>>;
type PendingMap = Arc<StdMutex<HashMap<String, oneshot::Sender<()>>>>;
type SessionHandle = Arc<AsyncMutex<Option<String>>>;

/// Deny message for non-question tools — same semantics the CLI applies on
/// its own in plain headless mode (ask-tier tools are never granted).
const TOOL_DENY_MESSAGE: &str = "Sigo runs claude as a translation layer; tool permissions are \
     not granted interactively. Pre-approve tools via permission settings or extra_args.";
const UNATTENDED_DENY_MESSAGE: &str =
    "No interactive handler is attached; the question cannot be answered.";
const DECLINED_DENY_MESSAGE: &str = "User declined to answer";

/// Handle to a running interactive `claude` process.
///
/// Dropping this kills the child (the reaper task observes the kill-switch
/// sender going away); [`shutdown`](Self::shutdown) does the same explicitly.
#[derive(Debug)]
pub(crate) struct InteractiveProc {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    turn_slot: Arc<StdMutex<Option<TurnSender>>>,
    alive: Arc<AtomicBool>,
    kill: Option<oneshot::Sender<()>>,
}

impl InteractiveProc {
    /// Whether the child process is still believed to be running.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Claim the (single) turn slot, returning the receiver for this turn's
    /// chunks. Errors if a turn is already in flight — the interactive
    /// process is strictly one-turn-at-a-time.
    pub(crate) fn begin_turn(&self) -> Result<mpsc::Receiver<Result<ResponseChunk>>> {
        let mut slot = self.turn_slot.lock().unwrap();
        if slot.is_some() {
            return Err(SigoError::Backend(
                "interactive claude-code backend supports one turn at a time \
                 (control_mode=full cannot run a parallel English turn here)"
                    .into(),
            ));
        }
        let (tx, rx) = mpsc::channel(64);
        *slot = Some(tx);
        Ok(rx)
    }

    /// Release the turn slot without completing the turn (used when the
    /// user-turn write fails before any event arrived).
    pub(crate) fn abort_turn(&self) {
        self.turn_slot.lock().unwrap().take();
    }

    /// Write one NDJSON line to the child's stdin.
    pub(crate) async fn write_line(&self, line: &str) -> Result<()> {
        write_line_to(&self.stdin, line).await
    }

    /// Kill the child process.
    pub(crate) fn shutdown(mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
    }
}

async fn write_line_to(stdin: &Arc<AsyncMutex<ChildStdin>>, line: &str) -> Result<()> {
    let mut s = stdin.lock().await;
    let io = async {
        s.write_all(line.as_bytes()).await?;
        s.write_all(b"\n").await?;
        s.flush().await
    };
    io.await
        .map_err(|e| SigoError::Backend(format!("write to claude stdin: {e}")))
}

/// The interactive-mode argv (everything after the binary name).
pub(crate) fn interactive_args(
    model: Option<&str>,
    resume: Option<&str>,
    extra: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    if let Some(sid) = resume {
        args.push("--resume".into());
        args.push(sid.into());
    }
    args.extend(extra.iter().cloned());
    args
}

/// Spawn the long-lived interactive process and its pump/reaper tasks.
///
/// `session` is shared with the per-turn mode: the captured session id makes
/// crash recovery (`--resume`) and mode switches seamless.
pub(crate) async fn spawn_interactive(
    binary: &str,
    model: Option<&str>,
    extra_args: &[String],
    session: SessionHandle,
    question_tx: mpsc::Sender<QuestionRequest>,
) -> Result<InteractiveProc> {
    let resume = session.lock().await.clone();
    let mut cmd = Command::new(binary);
    cmd.args(interactive_args(model, resume.as_deref(), extra_args));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| SigoError::Backend(format!("spawn {binary}: {e}")))?;
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| SigoError::Backend("no stdin on claude child".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SigoError::Backend("no stdout on claude child".into()))?;
    let stderr = child.stderr.take();

    let stdin = Arc::new(AsyncMutex::new(child_stdin));
    let turn_slot: Arc<StdMutex<Option<TurnSender>>> = Arc::default();
    let alive = Arc::new(AtomicBool::new(true));
    let stderr_buf = Arc::new(StdMutex::new(String::new()));

    // Drain stderr concurrently (an unread pipe can fill and deadlock the
    // child); it is also the only diagnostic when the process dies mid-turn.
    if let Some(se) = stderr {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(se).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut b = buf.lock().unwrap();
                b.push_str(&l);
                b.push('\n');
            }
        });
    }

    // Reaper: owns the child. Kills it when the kill switch fires OR when the
    // InteractiveProc handle is dropped (sender dropped → recv errors).
    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let killed = tokio::select! {
            _ = kill_rx => true,
            _ = child.wait() => false,
        };
        if killed {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    });

    // Pump: routes stdout lines to the current turn and answers control requests.
    let pump_stdin = stdin.clone();
    let pump_slot = turn_slot.clone();
    let pump_alive = alive.clone();
    let pending: PendingMap = Arc::default();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        // Both EOF (Ok(None)) and a read error end the pump.
        while let Ok(Some(line)) = reader.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            handle_line(
                &line,
                &pump_stdin,
                &pump_slot,
                &session,
                &question_tx,
                &pending,
            )
            .await;
        }
        pump_alive.store(false, Ordering::SeqCst);
        // A turn still in flight at EOF is a hard error: the orchestrator
        // marks it incomplete and history does not advance.
        let in_flight = pump_slot.lock().unwrap().take();
        if let Some(tx) = in_flight {
            let detail = stderr_buf.lock().unwrap().trim().to_string();
            let msg = if detail.is_empty() {
                "claude interactive process exited mid-turn".to_string()
            } else {
                format!("claude interactive process exited mid-turn: {detail}")
            };
            let _ = tx.send(Err(SigoError::Backend(msg))).await;
        }
    });

    Ok(InteractiveProc {
        stdin,
        turn_slot,
        alive,
        kill: Some(kill_tx),
    })
}

async fn handle_line(
    line: &str,
    stdin: &Arc<AsyncMutex<ChildStdin>>,
    turn_slot: &Arc<StdMutex<Option<TurnSender>>>,
    session: &SessionHandle,
    question_tx: &mpsc::Sender<QuestionRequest>,
    pending: &PendingMap,
) {
    if let Some(ev) = parse_control_line(line) {
        match ev {
            ControlEvent::CanUseTool(req) => {
                handle_can_use_tool(req, stdin, question_tx, pending).await;
            }
            ControlEvent::Cancel { request_id } => {
                if let Some(retire) = pending.lock().unwrap().remove(&request_id) {
                    let _ = retire.send(());
                }
            }
        }
        return;
    }
    match parse_line(line, session).await {
        Ok(Some(chunk)) => {
            // Take the sender BEFORE delivering Done so the next begin_turn
            // can never observe a stale in-flight slot.
            let done = matches!(chunk, ResponseChunk::Done { .. });
            let tx = {
                let mut slot = turn_slot.lock().unwrap();
                if done {
                    slot.take()
                } else {
                    slot.clone()
                }
            };
            if let Some(tx) = tx {
                let _ = tx.send(Ok(chunk)).await;
            }
        }
        Ok(None) => {}
        Err(e) => {
            // A malformed line fails the CURRENT turn only; the process may
            // still be healthy for subsequent turns.
            let tx = turn_slot.lock().unwrap().take();
            if let Some(tx) = tx {
                let _ = tx.send(Err(e)).await;
            }
        }
    }
}

async fn handle_can_use_tool(
    req: CanUseTool,
    stdin: &Arc<AsyncMutex<ChildStdin>>,
    question_tx: &mpsc::Sender<QuestionRequest>,
    pending: &PendingMap,
) {
    if req.tool_name != "AskUserQuestion" {
        let _ = write_line_to(stdin, &build_deny_line(&req.request_id, TOOL_DENY_MESSAGE)).await;
        return;
    }
    let questions = parse_questions(&req.input);
    if questions.is_empty() {
        let _ = write_line_to(
            stdin,
            &build_deny_line(&req.request_id, "Malformed AskUserQuestion input"),
        )
        .await;
        return;
    }

    // Register the retire switch BEFORE the pump reads any further lines, so
    // a control_cancel_request can never race past us.
    let (retire_tx, retire_rx) = oneshot::channel::<()>();
    pending
        .lock()
        .unwrap()
        .insert(req.request_id.clone(), retire_tx);

    let stdin = stdin.clone();
    let question_tx = question_tx.clone();
    let pending = pending.clone();
    tokio::spawn(async move {
        let (reply_tx, reply_rx) = oneshot::channel::<QuestionReply>();
        let sent = question_tx
            .send(QuestionRequest {
                questions,
                reply: reply_tx,
            })
            .await;
        let line = if sent.is_err() {
            Some(build_deny_line(&req.request_id, UNATTENDED_DENY_MESSAGE))
        } else {
            tokio::select! {
                // Retired by the CLI: drop the reply channel (the handler's
                // send fails / `closed()` resolves) and answer nothing.
                _ = retire_rx => None,
                r = reply_rx => Some(match r {
                    Ok(QuestionReply::Answers(answers)) => {
                        build_allow_line(&req.request_id, &req.input, &answers)
                    }
                    Ok(QuestionReply::Decline) | Err(_) => {
                        build_deny_line(&req.request_id, DECLINED_DENY_MESSAGE)
                    }
                }),
            }
        };
        if let Some(l) = line {
            let _ = write_line_to(&stdin, &l).await;
        }
        pending.lock().unwrap().remove(&req.request_id);
    });
}

#[cfg(test)]
mod tests {
    use crate::claude::question::{QuestionAnswer, QuestionReply, QuestionRequest};
    use crate::claude::{ClaudeBackend, ClaudeCodeBackend, ResponseChunk};
    use crate::conversation::Conversation;
    use futures::StreamExt;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    #[cfg(unix)]
    fn write_exec(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// Build a fake `claude` as a python3 script. Each spawn appends its argv to
    /// `marker`; `body` drives the NDJSON conversation. Helpers available to the
    /// body: `emit(obj)` writes one NDJSON line; `read()` reads one (exits 0 on EOF).
    #[cfg(unix)]
    fn py_script(marker: &Path, body: &str) -> String {
        format!(
            "#!/usr/bin/env python3\n\
             import sys, json\n\
             open({marker:?}, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n\
             def emit(o):\n\
             \tsys.stdout.write(json.dumps(o) + '\\n')\n\
             \tsys.stdout.flush()\n\
             def read():\n\
             \tline = sys.stdin.readline()\n\
             \tif not line:\n\
             \t\tsys.exit(0)\n\
             \treturn json.loads(line)\n\
             {body}\n",
            marker = marker.to_str().unwrap(),
        )
    }

    const ROUND_TRIP_BODY: &str = r#"
m = read()
assert m['type'] == 'user', m
emit({'type':'system','subtype':'init','session_id':'sess-1'})
emit({'type':'control_request','request_id':'req-1','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','tool_use_id':'t1','input':{'questions':[{'question':'你最喜欢什么颜色？','header':'颜色','options':[{'label':'蓝色','description':'冷静'},{'label':'红色','description':'热情'}],'multiSelect':False}]}}})
r = read()
assert r['type'] == 'control_response', r
assert r['response']['request_id'] == 'req-1', r
resp = r['response']['response']
assert resp['behavior'] == 'allow', r
assert resp['updatedInput']['questions'][0]['question'] == '你最喜欢什么颜色？', r
ans = resp['updatedInput']['answers']['你最喜欢什么颜色？']
emit({'type':'assistant','message':{'content':[{'type':'text','text':'你选了' + ans + '。'}]}})
emit({'type':'result','usage':{'input_tokens':3,'output_tokens':4},'session_id':'sess-1'})
m2 = read()
assert m2['type'] == 'user', m2
emit({'type':'assistant','message':{'content':[{'type':'text','text':'第二回合：' + m2['message']['content']}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1},'session_id':'sess-1'})
read()
"#;

    /// Spawn an auto-answering handler: records every QuestionRequest's
    /// questions and replies with the given closure's answers.
    fn auto_answer(
        mut rx: mpsc::Receiver<QuestionRequest>,
        answer_label: &'static str,
    ) -> Arc<Mutex<Vec<Vec<crate::claude::AskQuestion>>>> {
        let seen: Arc<Mutex<Vec<Vec<crate::claude::AskQuestion>>>> = Arc::default();
        let seen2 = seen.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen2.lock().unwrap().push(req.questions.clone());
                let answers = req
                    .questions
                    .iter()
                    .map(|q| QuestionAnswer {
                        question: q.question.clone(),
                        answer: answer_label.to_string(),
                    })
                    .collect();
                let _ = req.reply.send(QuestionReply::Answers(answers));
            }
        });
        seen
    }

    /// Open a turn stream, retrying the well-known fork/exec ETXTBSY race:
    /// a concurrently-forked test child can briefly hold the just-written
    /// script's write-fd until its own exec completes.
    async fn open_stream_retrying(
        backend: &ClaudeCodeBackend,
        prompt: &str,
    ) -> futures::stream::BoxStream<'static, crate::error::Result<ResponseChunk>> {
        for _ in 0..40 {
            match backend.stream_turn(&Conversation::new(), prompt).await {
                Ok(s) => return s,
                Err(e) if e.to_string().contains("Text file busy") => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(e) => panic!("stream_turn: {e}"),
            }
        }
        panic!("ETXTBSY persisted after retries");
    }

    async fn collect_turn(
        backend: &ClaudeCodeBackend,
        prompt: &str,
    ) -> (String, Option<crate::conversation::Usage>, Vec<String>) {
        let mut stream = open_stream_retrying(backend, prompt).await;
        let mut text = String::new();
        let mut usage = None;
        let mut errors = vec![];
        while let Some(item) = stream.next().await {
            match item {
                Ok(ResponseChunk::TextDelta(t)) => text.push_str(&t),
                Ok(ResponseChunk::Done { usage: u, .. }) => {
                    usage = Some(u);
                    break;
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        (text, usage, errors)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn question_round_trip_and_second_turn_reuse_one_process() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        write_exec(&script, &py_script(&marker, ROUND_TRIP_BODY));

        let (qtx, qrx) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);
        let seen = auto_answer(qrx, "蓝色");

        // Turn 1: the question round-trips and Claude continues in the SAME turn.
        let (text, usage, errors) = collect_turn(&backend, "你好").await;
        assert!(errors.is_empty(), "turn 1 errors: {errors:?}");
        assert_eq!(text, "你选了蓝色。");
        assert_eq!(usage.unwrap().input_tokens, 3);

        // The handler saw the question exactly as the model asked it.
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0][0].question, "你最喜欢什么颜色？");
        assert_eq!(seen[0][0].header, "颜色");
        assert_eq!(seen[0][0].options[1].label, "红色");

        // Turn 2 rides the same process (the script answers it from the same flow).
        let (text2, usage2, errors2) = collect_turn(&backend, "继续").await;
        assert!(errors2.is_empty(), "turn 2 errors: {errors2:?}");
        assert_eq!(text2, "第二回合：继续");
        assert!(usage2.is_some());

        // Exactly one spawn, with the interactive argv.
        let spawns = std::fs::read_to_string(&marker).unwrap();
        let lines: Vec<&str> = spawns.lines().collect();
        assert_eq!(lines.len(), 1, "expected one spawn, got: {spawns}");
        for flag in [
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--permission-prompt-tool stdio",
        ] {
            assert!(
                lines[0].contains(flag),
                "argv missing `{flag}`: {}",
                lines[0]
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn declined_question_sends_deny_and_turn_continues() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        let body = r#"
m = read()
emit({'type':'control_request','request_id':'req-1','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','tool_use_id':'t1','input':{'questions':[{'question':'继续吗？','options':[{'label':'是','description':''},{'label':'否','description':''}]}]}}})
r = read()
resp = r['response']['response']
assert resp['behavior'] == 'deny', r
assert resp['message'], r
emit({'type':'assistant','message':{'content':[{'type':'text','text':'好的，不问了。'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1}})
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, mut qrx) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);
        tokio::spawn(async move {
            while let Some(req) = qrx.recv().await {
                let _ = req.reply.send(QuestionReply::Decline);
            }
        });

        let (text, usage, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(text, "好的，不问了。");
        assert!(usage.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_handler_means_deny_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        let body = r#"
m = read()
emit({'type':'control_request','request_id':'req-1','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','tool_use_id':'t1','input':{'questions':[{'question':'问？','options':[{'label':'甲','description':''},{'label':'乙','description':''}]}]}}})
r = read()
assert r['response']['response']['behavior'] == 'deny', r
emit({'type':'assistant','message':{'content':[{'type':'text','text':'未答复'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1}})
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, qrx) = mpsc::channel::<QuestionRequest>(4);
        drop(qrx); // nobody is listening
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);

        let (text, _usage, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(text, "未答复");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_question_tools_are_denied_preserving_headless_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        let body = r#"
m = read()
emit({'type':'control_request','request_id':'req-7','request':{'subtype':'can_use_tool','tool_name':'Bash','tool_use_id':'t1','input':{'command':'rm -rf /'}}})
r = read()
assert r['response']['request_id'] == 'req-7', r
assert r['response']['response']['behavior'] == 'deny', r
emit({'type':'assistant','message':{'content':[{'type':'text','text':'工具被拒'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1}})
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, qrx) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);
        let seen = auto_answer(qrx, "unused");

        let (text, _usage, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(text, "工具被拒");
        // The Bash permission request must never reach the question handler.
        assert!(seen.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_question_is_retired_and_handler_notified() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        let body = r#"
m = read()
emit({'type':'control_request','request_id':'req-1','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','tool_use_id':'t1','input':{'questions':[{'question':'问？','options':[{'label':'甲','description':''},{'label':'乙','description':''}]}]}}})
emit({'type':'control_cancel_request','request_id':'req-1'})
emit({'type':'assistant','message':{'content':[{'type':'text','text':'撤回了'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1}})
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, mut qrx) = mpsc::channel::<QuestionRequest>(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);

        // Handler that never answers; after the cancel, its reply channel must
        // report closure (send fails) instead of leaving the CLI waiting.
        let cancelled = Arc::new(Mutex::new(false));
        let cancelled2 = cancelled.clone();
        tokio::spawn(async move {
            if let Some(mut req) = qrx.recv().await {
                // wait for the reply channel to be closed by the cancel
                req.reply.closed().await;
                *cancelled2.lock().unwrap() = true;
            }
        });

        let (text, _usage, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(text, "撤回了");
        // Give the handler task a beat to observe the closure.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            *cancelled.lock().unwrap(),
            "handler must learn its question was retired"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn crash_mid_turn_errors_then_respawn_resumes_session() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        // Turn 1 completes cleanly (capturing session sess-9), then the process
        // exits. The next turn must respawn with --resume sess-9.
        let body = r#"
m = read()
emit({'type':'system','subtype':'init','session_id':'sess-9'})
emit({'type':'assistant','message':{'content':[{'type':'text','text':'回合一'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1},'session_id':'sess-9'})
"#; // script exits here -> EOF between turns
        write_exec(&script, &py_script(&marker, body));

        let (qtx, _qrx_keepalive) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);

        let (text, usage, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "turn 1 errors: {errors:?}");
        assert_eq!(text, "回合一");
        assert!(usage.is_some());

        // Let the script's process actually die (otherwise turn 2's stdin
        // write can still land in the pipe buffer, which is a genuine
        // mid-turn death — a different, also-handled scenario).
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Second turn: the dead process must be detected and respawned.
        let (text2, _usage2, _errors2) = collect_turn(&backend, "again").await;
        assert_eq!(text2, "回合一"); // fresh script run answers the same way

        let spawns = std::fs::read_to_string(&marker).unwrap();
        let lines: Vec<&str> = spawns.lines().collect();
        assert_eq!(lines.len(), 2, "expected respawn, got: {spawns}");
        assert!(
            lines[1].contains("--resume sess-9"),
            "respawn must resume the captured session: {}",
            lines[1]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reset_session_kills_process_and_forgets_session() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        let body = r#"
m = read()
emit({'type':'system','subtype':'init','session_id':'sess-1'})
emit({'type':'assistant','message':{'content':[{'type':'text','text':'回合'}]}})
emit({'type':'result','usage':{'input_tokens':1,'output_tokens':1},'session_id':'sess-1'})
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, _qrx_keepalive) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);

        let (text, _u, errors) = collect_turn(&backend, "hi").await;
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(text, "回合");

        backend.reset_session().await;

        let (text2, _u2, errors2) = collect_turn(&backend, "hi").await;
        assert!(errors2.is_empty(), "errors after reset: {errors2:?}");
        assert_eq!(text2, "回合");

        let spawns = std::fs::read_to_string(&marker).unwrap();
        let lines: Vec<&str> = spawns.lines().collect();
        assert_eq!(lines.len(), 2, "reset must force a fresh spawn: {spawns}");
        assert!(
            !lines[1].contains("--resume"),
            "after reset the new process must NOT resume the old session: {}",
            lines[1]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_turn_on_same_interactive_process_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawns");
        let script = dir.path().join("fake-claude");
        // Reads turn 1's user line and then stalls (no result) until stdin EOF.
        let body = r#"
m = read()
emit({'type':'system','subtype':'init','session_id':'s'})
read()
read()
"#;
        write_exec(&script, &py_script(&marker, body));

        let (qtx, _qrx_keepalive) = mpsc::channel(4);
        let backend = ClaudeCodeBackend::new(script.to_str().unwrap()).with_question_channel(qtx);

        let _stream1 = open_stream_retrying(&backend, "first").await;
        let second = backend.stream_turn(&Conversation::new(), "second").await;
        match second {
            Err(e) => assert!(
                e.to_string().contains("one turn at a time"),
                "wrong rejection: {e}"
            ),
            Ok(_) => panic!("a second concurrent turn must be rejected, not interleaved"),
        }
    }
}
