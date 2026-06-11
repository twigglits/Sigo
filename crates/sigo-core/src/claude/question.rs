//! AskUserQuestion passthrough types and stream-json control-protocol JSON.
//!
//! When the `claude` CLI runs with `--permission-prompt-tool stdio`, tool
//! permission decisions arrive on stdout as `control_request` lines and are
//! answered by writing `control_response` lines to the CLI's stdin. This
//! module holds the pure data types and (de)serialization for that exchange —
//! no I/O. Shapes were captured live from claude 2.1.173 (see the design doc
//! `docs/superpowers/specs/2026-06-11-interactive-question-passthrough-design.md`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// One selectable option of an [`AskQuestion`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    /// Short display label (the model's own words — echoed back verbatim on answer).
    pub label: String,
    /// Explanation of what choosing this option means.
    #[serde(default)]
    pub description: String,
}

/// One clarification question from Claude Code's `AskUserQuestion` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    /// The complete question text (answer keys must match this byte-for-byte).
    pub question: String,
    /// Very short chip/tag label (≤12 chars advisory).
    #[serde(default)]
    pub header: String,
    /// 2–4 options offered by the model.
    #[serde(default)]
    pub options: Vec<AskOption>,
    /// Whether multiple options may be selected (answers are ", "-joined).
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// One resolved answer: the original question text mapped to the chosen
/// option label(s) (", "-joined when multi-select) or free text.
///
/// Both fields must be in the conversation's own language (Chinese in Sigo's
/// pipeline): the CLI matches `question` byte-for-byte and the model expects
/// its own option labels back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// The original, untranslated question text.
    pub question: String,
    /// The chosen label(s) or free text.
    pub answer: String,
}

/// The user's decision on a [`QuestionRequest`].
#[derive(Debug)]
pub enum QuestionReply {
    /// One answer per question, keyed by the original question text.
    Answers(Vec<QuestionAnswer>),
    /// The user declined to answer.
    Decline,
}

/// A pending interactive question surfaced by the backend mid-turn.
///
/// The handler (e.g. the REPL's picker bridge) must send exactly one
/// [`QuestionReply`] on `reply`. Dropping the sender counts as a decline.
#[derive(Debug)]
pub struct QuestionRequest {
    /// The questions exactly as the model asked them (untranslated).
    pub questions: Vec<AskQuestion>,
    /// One-shot reply channel back to the backend.
    pub reply: oneshot::Sender<QuestionReply>,
}

/// A `can_use_tool` permission request parsed from a `control_request` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanUseTool {
    /// Correlation id to echo in the response.
    pub request_id: String,
    /// Name of the tool seeking permission (e.g. `"AskUserQuestion"`).
    pub tool_name: String,
    /// The tool's full input — echoed verbatim (plus answers) on allow.
    pub input: Value,
}

/// One inbound control-channel event relevant to Sigo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// The CLI asks permission to run a tool (blocks its turn awaiting a response).
    CanUseTool(CanUseTool),
    /// The CLI retired a pending request; it must not be answered anymore.
    Cancel {
        /// The id of the retired request.
        request_id: String,
    },
}

/// Parse one NDJSON line into a [`ControlEvent`], if it is one.
///
/// Returns `None` for every non-control line (assistant/system/result/...),
/// for control subtypes Sigo does not handle, and for unparseable input —
/// callers fall through to the ordinary event parser.
pub fn parse_control_line(line: &str) -> Option<ControlEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type").and_then(Value::as_str)? {
        "control_request" => {
            let request_id = v.get("request_id")?.as_str()?.to_string();
            let request = v.get("request")?;
            if request.get("subtype").and_then(Value::as_str)? != "can_use_tool" {
                return None;
            }
            Some(ControlEvent::CanUseTool(CanUseTool {
                request_id,
                tool_name: request.get("tool_name")?.as_str()?.to_string(),
                input: request.get("input").cloned().unwrap_or(Value::Null),
            }))
        }
        "control_cancel_request" => Some(ControlEvent::Cancel {
            request_id: v.get("request_id")?.as_str()?.to_string(),
        }),
        _ => None,
    }
}

/// Extract the [`AskQuestion`]s from a `can_use_tool` input value.
///
/// Unknown fields are tolerated; a missing/malformed `questions` array yields
/// an empty vec (the caller should then deny rather than guess).
pub fn parse_questions(input: &Value) -> Vec<AskQuestion> {
    input
        .get("questions")
        .and_then(|q| serde_json::from_value(q.clone()).ok())
        .unwrap_or_default()
}

/// Build the stdin line allowing a tool call, echoing `original_input`
/// verbatim with the user's `answers` merged in.
///
/// The echo is built from the raw [`Value`] (not re-serialized through our
/// structs) so fields Sigo does not model — `metadata`, future additions —
/// survive byte-for-byte.
pub fn build_allow_line(
    request_id: &str,
    original_input: &Value,
    answers: &[QuestionAnswer],
) -> String {
    let mut updated = original_input.clone();
    if !answers.is_empty() {
        let map: serde_json::Map<String, Value> = answers
            .iter()
            .map(|a| (a.question.clone(), Value::String(a.answer.clone())))
            .collect();
        if let Some(obj) = updated.as_object_mut() {
            obj.insert("answers".to_string(), Value::Object(map));
        }
    }
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "allow", "updatedInput": updated }
        }
    })
    .to_string()
}

/// Build the stdin line denying a tool call with a human-readable reason.
pub fn build_deny_line(request_id: &str, message: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": { "behavior": "deny", "message": message, "interrupt": false }
        }
    })
    .to_string()
}

/// Build the stdin line that starts a new user turn on a long-lived
/// stream-json `claude` process.
pub fn build_user_turn_line(prompt: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": prompt }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact shape captured live from claude 2.1.173 with
    /// `--permission-prompt-tool stdio` (request_id/session ids shortened).
    const LIVE_CONTROL_REQUEST: &str = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"你最喜欢什么颜色？","header":"颜色","options":[{"label":"蓝色","description":"冷静"},{"label":"红色","description":"热情"}],"multiSelect":false}]},"tool_use_id":"toolu_01"}}"#;

    #[test]
    fn parses_live_can_use_tool_request() {
        let ev = parse_control_line(LIVE_CONTROL_REQUEST).expect("control line must parse");
        let ControlEvent::CanUseTool(req) = ev else {
            panic!("expected CanUseTool, got {ev:?}");
        };
        assert_eq!(req.request_id, "req-1");
        assert_eq!(req.tool_name, "AskUserQuestion");
        let qs = parse_questions(&req.input);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].question, "你最喜欢什么颜色？");
        assert_eq!(qs[0].header, "颜色");
        assert!(!qs[0].multi_select);
        assert_eq!(qs[0].options.len(), 2);
        assert_eq!(qs[0].options[0].label, "蓝色");
        assert_eq!(qs[0].options[0].description, "冷静");
    }

    #[test]
    fn parses_cancel_request() {
        let line = r#"{"type":"control_cancel_request","request_id":"req-9"}"#;
        assert_eq!(
            parse_control_line(line),
            Some(ControlEvent::Cancel {
                request_id: "req-9".into()
            })
        );
    }

    #[test]
    fn non_control_lines_fall_through() {
        for line in [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"result"}"#,
            // unsupported control subtype must also fall through (ignored)
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"elicitation"}}"#,
            "not json at all",
        ] {
            assert_eq!(parse_control_line(line), None, "line: {line}");
        }
    }

    #[test]
    fn question_parsing_tolerates_missing_optional_fields_and_extras() {
        let input = serde_json::json!({
            "questions": [
                {"question": "继续吗？", "options": [{"label": "是"}, {"label": "否"}], "unknown_field": 42}
            ],
            "metadata": {"source": "test"}
        });
        let qs = parse_questions(&input);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].header, "");
        assert!(!qs[0].multi_select);
        assert_eq!(qs[0].options[0].description, "");
    }

    #[test]
    fn malformed_questions_yield_empty_not_panic() {
        assert!(parse_questions(&serde_json::json!({})).is_empty());
        assert!(parse_questions(&serde_json::json!({"questions": "nope"})).is_empty());
        assert!(parse_questions(&Value::Null).is_empty());
    }

    #[test]
    fn allow_line_echoes_input_verbatim_and_merges_answers() {
        let ev = parse_control_line(LIVE_CONTROL_REQUEST).unwrap();
        let ControlEvent::CanUseTool(req) = ev else {
            unreachable!()
        };
        let line = build_allow_line(
            &req.request_id,
            &req.input,
            &[QuestionAnswer {
                question: "你最喜欢什么颜色？".into(),
                answer: "蓝色".into(),
            }],
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-1");
        let inner = &v["response"]["response"];
        assert_eq!(inner["behavior"], "allow");
        // questions echoed byte-for-byte (Value equality)
        assert_eq!(inner["updatedInput"]["questions"], req.input["questions"]);
        assert_eq!(
            inner["updatedInput"]["answers"]["你最喜欢什么颜色？"],
            "蓝色"
        );
        // single line — must be writable as one NDJSON line
        assert!(!line.contains('\n'));
    }

    #[test]
    fn allow_line_preserves_unmodelled_input_fields() {
        let input = serde_json::json!({"questions": [], "metadata": {"k": "v"}});
        let line = build_allow_line("r", &input, &[]);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v["response"]["response"]["updatedInput"]["metadata"]["k"],
            "v"
        );
    }

    #[test]
    fn deny_line_shape() {
        let line = build_deny_line("req-2", "User declined to answer");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["request_id"], "req-2");
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        assert_eq!(
            v["response"]["response"]["message"],
            "User declined to answer"
        );
        assert_eq!(v["response"]["response"]["interrupt"], false);
        assert!(!line.contains('\n'));
    }

    #[test]
    fn user_turn_line_shape_and_escaping() {
        let line = build_user_turn_line("hello\n\"world\"");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hello\n\"world\"");
        assert!(!line.contains('\n'), "newlines must be escaped: {line}");
    }

    #[test]
    fn multi_select_round_trip() {
        let input = serde_json::json!({"questions": [
            {"question": "选哪些？", "header": "多选", "multiSelect": true,
             "options": [{"label": "甲", "description": ""}, {"label": "乙", "description": ""}]}
        ]});
        let qs = parse_questions(&input);
        assert!(qs[0].multi_select);
        // a multi-select answer is the ", "-joined labels (verified live shape)
        let line = build_allow_line(
            "r",
            &input,
            &[QuestionAnswer {
                question: "选哪些？".into(),
                answer: "甲, 乙".into(),
            }],
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v["response"]["response"]["updatedInput"]["answers"]["选哪些？"],
            "甲, 乙"
        );
    }
}
