use chrono::Utc;
use futures::StreamExt;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use crate::benchmark::{BenchmarkSink, EnglishControlRun, TurnRecord, SCHEMA_VERSION};
use crate::claude::{ClaudeBackend, ResponseChunk};
use crate::conversation::{BackendKind, Conversation, Direction};
use crate::error::Result;
use crate::stream::{Segment, SentenceBuffer};
use crate::tokenizer::Tokenizer;
use crate::translator::Translator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMode {
    Off,
    PromptOnly,
    Full,
}

impl ControlMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "prompt-only" => Some(Self::PromptOnly),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

pub struct OrchestratorConfig {
    pub backend_kind: BackendKind,
    pub claude_model: String,
    pub translator_model: String,
    pub control_mode: ControlMode,
}

pub struct Orchestrator {
    pub session_id: Uuid,
    pub turn_index: u32,
    pub chinese_convo: Conversation,
    pub english_convo: Conversation,
    pub config: OrchestratorConfig,
    pub translator: Arc<dyn Translator>,
    pub backend: Arc<dyn ClaudeBackend>,
    pub tokenizer: Arc<dyn Tokenizer>,
    pub sink: Arc<dyn BenchmarkSink>,
}

/// Sink for streamed English output. Implementations: terminal printer, in-memory buffer for tests.
pub trait OutputSink: Send {
    fn write(&mut self, s: &str);
    fn flush(&mut self) {}
}

pub struct StdoutSink;
impl OutputSink for StdoutSink {
    fn write(&mut self, s: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }
}

#[derive(Default)]
pub struct CollectSink {
    pub buf: String,
}
impl OutputSink for CollectSink {
    fn write(&mut self, s: &str) {
        self.buf.push_str(s);
    }
}

impl Orchestrator {
    pub fn new(
        config: OrchestratorConfig,
        translator: Arc<dyn Translator>,
        backend: Arc<dyn ClaudeBackend>,
        tokenizer: Arc<dyn Tokenizer>,
        sink: Arc<dyn BenchmarkSink>,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4(),
            turn_index: 0,
            chinese_convo: Conversation::new(),
            english_convo: Conversation::new(),
            config,
            translator,
            backend,
            tokenizer,
            sink,
        }
    }

    pub fn reset(&mut self) {
        self.session_id = Uuid::new_v4();
        self.turn_index = 0;
        self.chinese_convo = Conversation::new();
        self.english_convo = Conversation::new();
    }

    /// Run one full turn end-to-end. The streamed English output is written to `out`.
    /// Returns the recorded TurnRecord on success. On failure (incomplete stream), conversation
    /// state is unchanged; the record is still appended to the sink with `incomplete = true`.
    pub async fn run_turn(
        &mut self,
        english_prompt: &str,
        out: &mut dyn OutputSink,
    ) -> Result<TurnRecord> {
        let turn_started = Instant::now();
        let mut errors: Vec<String> = vec![];

        // Step 1: EN → ZH
        let translation_in_started = Instant::now();
        let chinese_prompt = self
            .translator
            .translate(english_prompt, Direction::EnToZh)
            .await?;
        let translation_in_ms = translation_in_started.elapsed().as_millis() as u64;

        // Step 2: Local token counts for both prompts.
        let english_prompt_tokens_local = self
            .tokenizer
            .count_tokens(english_prompt)
            .unwrap_or_else(|e| {
                errors.push(format!("tokenizer en prompt: {e}"));
                0
            });
        let chinese_prompt_tokens_local = self
            .tokenizer
            .count_tokens(&chinese_prompt)
            .unwrap_or_else(|e| {
                errors.push(format!("tokenizer zh prompt: {e}"));
                0
            });

        // Cumulative totals: prior session content + this prompt.
        let chinese_cumulative_prompt_tokens_local =
            cumulative_tokens(self.tokenizer.as_ref(), &self.chinese_convo)
                + chinese_prompt_tokens_local;
        let english_cumulative_prompt_tokens_local =
            cumulative_tokens(self.tokenizer.as_ref(), &self.english_convo)
                + english_prompt_tokens_local;

        // Step 2.5 (Full control mode only): launch parallel English Claude run.
        let english_control_future: Option<tokio::task::JoinHandle<Result<EnglishControlRun>>> =
            if self.config.control_mode == ControlMode::Full {
                let backend = self.backend.clone();
                let en_convo = self.english_convo.clone();
                let en_prompt = english_prompt.to_string();
                Some(tokio::spawn(async move {
                    run_english_control(backend, en_convo, en_prompt).await
                }))
            } else {
                None
            };

        // Step 3: Open Claude stream — conversation history does NOT include the new prompt yet.
        let mut stream = self
            .backend
            .stream_turn(&self.chinese_convo, &chinese_prompt)
            .await?;

        // Step 4: Stream consumption with sentence buffering and per-segment translation.
        let mut chinese_response = String::new();
        let mut english_response_emitted = String::new();
        let mut buffer = SentenceBuffer::new();
        let mut reported_input: Option<u32> = None;
        let mut reported_output: Option<u32> = None;
        let mut cache_read: Option<u32> = None;
        let mut cache_write: Option<u32> = None;
        let mut ttft_ms: u64 = 0;
        let claude_started = Instant::now();
        let mut got_first_delta = false;
        let mut translation_out_ms_total: u64 = 0;
        let mut translation_out_calls: u32 = 0;
        let mut incomplete = false;
        let mut stream_ended_with_done = false;

        while let Some(item) = stream.next().await {
            match item {
                Ok(ResponseChunk::TextDelta(text)) => {
                    if !got_first_delta {
                        ttft_ms = claude_started.elapsed().as_millis() as u64;
                        got_first_delta = true;
                    }
                    chinese_response.push_str(&text);
                    let segments = buffer.push(&text);
                    emit_segments(
                        self.translator.as_ref(),
                        segments,
                        &mut english_response_emitted,
                        &mut translation_out_ms_total,
                        &mut translation_out_calls,
                        &mut errors,
                        out,
                    )
                    .await;
                }
                Ok(ResponseChunk::Done {
                    usage,
                    stop_reason: _,
                }) => {
                    reported_input = Some(usage.input_tokens);
                    reported_output = Some(usage.output_tokens);
                    cache_read = usage.cache_read;
                    cache_write = usage.cache_write;
                    let tail = buffer.flush();
                    emit_segments(
                        self.translator.as_ref(),
                        tail,
                        &mut english_response_emitted,
                        &mut translation_out_ms_total,
                        &mut translation_out_calls,
                        &mut errors,
                        out,
                    )
                    .await;
                    stream_ended_with_done = true;
                    break;
                }
                Err(e) => {
                    let tail = buffer.flush();
                    emit_segments(
                        self.translator.as_ref(),
                        tail,
                        &mut english_response_emitted,
                        &mut translation_out_ms_total,
                        &mut translation_out_calls,
                        &mut errors,
                        out,
                    )
                    .await;
                    incomplete = true;
                    errors.push(format!("claude stream: {e}"));
                    break;
                }
            }
        }
        if !stream_ended_with_done && !incomplete {
            let tail = buffer.flush();
            emit_segments(
                self.translator.as_ref(),
                tail,
                &mut english_response_emitted,
                &mut translation_out_ms_total,
                &mut translation_out_calls,
                &mut errors,
                out,
            )
            .await;
        }
        let claude_total_ms = claude_started.elapsed().as_millis() as u64;

        // Step 5: Response-side token count.
        let chinese_response_tokens_local = self
            .tokenizer
            .count_tokens(&chinese_response)
            .unwrap_or_else(|e| {
                errors.push(format!("tokenizer zh response: {e}"));
                0
            });

        // Step 6: Advance conversation state ONLY if turn completed cleanly.
        if !incomplete {
            self.chinese_convo.push_user(chinese_prompt.clone());
            self.chinese_convo.push_assistant(chinese_response.clone());
            self.english_convo.push_user(english_prompt.to_string());
            self.english_convo
                .push_assistant(english_response_emitted.clone());
        }

        let english_control_run = if let Some(handle) = english_control_future {
            match handle.await {
                Ok(Ok(r)) => Some(r),
                Ok(Err(e)) => {
                    errors.push(format!("control run: {e}"));
                    None
                }
                Err(e) => {
                    errors.push(format!("control join: {e}"));
                    None
                }
            }
        } else {
            None
        };

        // Step 7: Build + record TurnRecord (always, including incomplete turns).
        let record = TurnRecord {
            schema_version: SCHEMA_VERSION,
            session_id: self.session_id,
            turn_index: self.turn_index,
            timestamp: Utc::now(),
            backend: self.config.backend_kind,
            claude_model: self.config.claude_model.clone(),
            translator_model: self.config.translator_model.clone(),
            english_prompt: english_prompt.to_string(),
            chinese_prompt,
            chinese_response,
            english_response: english_response_emitted,
            english_prompt_tokens_local,
            chinese_prompt_tokens_local,
            chinese_response_tokens_local,
            chinese_prompt_tokens_reported: reported_input,
            chinese_response_tokens_reported: reported_output,
            cache_read_tokens_reported: cache_read,
            cache_write_tokens_reported: cache_write,
            chinese_cumulative_prompt_tokens_local,
            english_cumulative_prompt_tokens_local,
            english_control_run,
            incomplete,
            turn_errors: errors,
            translation_in_ms,
            translation_out_ms_total,
            translation_out_calls,
            claude_ttft_ms: ttft_ms,
            claude_total_ms,
            turn_total_ms: turn_started.elapsed().as_millis() as u64,
        };
        if let Err(e) = self.sink.record(&record) {
            tracing::warn!(error = %e, "benchmark sink failed");
        }

        if !incomplete {
            self.turn_index += 1;
        }
        Ok(record)
    }
}

fn cumulative_tokens(tokenizer: &dyn Tokenizer, convo: &Conversation) -> u32 {
    let mut total: u32 = 0;
    if let Some(s) = &convo.system {
        total += tokenizer.count_tokens(s).unwrap_or(0);
    }
    for m in &convo.messages {
        total += tokenizer.count_tokens(&m.content).unwrap_or(0);
    }
    total
}

async fn emit_segments(
    translator: &dyn Translator,
    segments: Vec<Segment>,
    english_acc: &mut String,
    translation_out_ms_total: &mut u64,
    translation_out_calls: &mut u32,
    errors: &mut Vec<String>,
    out: &mut dyn OutputSink,
) {
    for seg in segments {
        match seg {
            Segment::Passthrough(raw) => {
                out.write(&raw);
                english_acc.push_str(&raw);
            }
            Segment::Text(zh) => {
                let t0 = Instant::now();
                match translator.translate(&zh, Direction::ZhToEn).await {
                    Ok(en) => {
                        out.write(&en);
                        english_acc.push_str(&en);
                    }
                    Err(e) => {
                        errors.push(format!("zh->en segment translation: {e}"));
                        out.write(&zh);
                        english_acc.push_str(&zh);
                    }
                }
                *translation_out_ms_total += t0.elapsed().as_millis() as u64;
                *translation_out_calls += 1;
            }
        }
    }
    out.flush();
}

async fn run_english_control(
    backend: Arc<dyn ClaudeBackend>,
    en_convo: Conversation,
    en_prompt: String,
) -> Result<EnglishControlRun> {
    let started = Instant::now();
    let mut stream = backend.stream_turn(&en_convo, &en_prompt).await?;
    let mut text = String::new();
    let mut usage_input = 0u32;
    let mut usage_output = 0u32;
    let mut cache_read: Option<u32> = None;
    let mut cache_write: Option<u32> = None;
    while let Some(item) = stream.next().await {
        match item? {
            ResponseChunk::TextDelta(t) => text.push_str(&t),
            ResponseChunk::Done { usage, .. } => {
                usage_input = usage.input_tokens;
                usage_output = usage.output_tokens;
                cache_read = usage.cache_read;
                cache_write = usage.cache_write;
                break;
            }
        }
    }
    Ok(EnglishControlRun {
        english_response: text,
        prompt_tokens_reported: usage_input,
        response_tokens_reported: usage_output,
        cache_read_tokens_reported: cache_read,
        cache_write_tokens_reported: cache_write,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::MemorySink;
    use crate::claude::FakeBackend;
    use crate::conversation::Usage;
    use crate::tokenizer::TokenizerProxy;
    use crate::translator::FakeTranslator;

    fn build(
        translator: Arc<FakeTranslator>,
        backend: Arc<FakeBackend>,
        sink: Arc<MemorySink>,
    ) -> Orchestrator {
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::Api,
            claude_model: "claude-sonnet-4-6".into(),
            translator_model: "qwen3:14b".into(),
            control_mode: ControlMode::PromptOnly,
        };
        let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
        Orchestrator::new(cfg, translator, backend, tokenizer, sink)
    }

    #[tokio::test]
    async fn happy_path_advances_history_and_records_turn() {
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("Hello, world!", "你好，世界！");
        translator.add_zh_to_en("你好，世界！", "Hello, world!");

        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple(
            "你好，世界！",
            Usage {
                input_tokens: 5,
                output_tokens: 5,
                ..Default::default()
            },
        );

        let sink = Arc::new(MemorySink::new());
        let mut orch = build(translator, backend, sink.clone());

        let mut out = CollectSink::default();
        let record = orch.run_turn("Hello, world!", &mut out).await.unwrap();

        assert!(!record.incomplete);
        assert_eq!(record.chinese_prompt, "你好，世界！");
        assert!(record.chinese_response.contains("你好"));
        assert!(out.buf.contains("Hello, world!"));
        assert_eq!(orch.chinese_convo.messages.len(), 2);
        assert_eq!(orch.english_convo.messages.len(), 2);
        assert_eq!(orch.turn_index, 1);
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn cumulative_token_counts_grow_each_turn() {
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("hi", "你好");
        translator.add_zh_to_en("你好。", "Hi.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple("你好。", Usage::default());
        backend.enqueue_simple("你好。", Usage::default());
        let sink = Arc::new(MemorySink::new());
        let mut orch = build(translator, backend, sink.clone());
        let mut out = CollectSink::default();
        let r1 = orch.run_turn("hi", &mut out).await.unwrap();
        let r2 = orch.run_turn("hi", &mut out).await.unwrap();
        assert!(
            r2.chinese_cumulative_prompt_tokens_local > r1.chinese_cumulative_prompt_tokens_local
        );
        assert!(
            r2.english_cumulative_prompt_tokens_local > r1.english_cumulative_prompt_tokens_local
        );
    }

    #[tokio::test]
    async fn mid_stream_error_marks_turn_incomplete_and_holds_history() {
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("ping", "乒");
        translator.add_zh_to_en("乓", "Pong");

        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_error_after_chunk("乓", "simulated network drop");

        let sink = Arc::new(MemorySink::new());
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::Api,
            claude_model: "claude-sonnet-4-6".into(),
            translator_model: "fake".into(),
            control_mode: ControlMode::PromptOnly,
        };
        let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
        let mut orch = Orchestrator::new(cfg, translator, backend, tokenizer, sink.clone());

        let mut out = CollectSink::default();
        let record = orch.run_turn("ping", &mut out).await.unwrap();

        // Critical invariants: turn marked incomplete, history unchanged, sink received the record.
        assert!(
            record.incomplete,
            "turn should be marked incomplete after mid-stream error"
        );
        assert_eq!(
            orch.chinese_convo.messages.len(),
            0,
            "conversation history must not advance on error"
        );
        assert_eq!(orch.english_convo.messages.len(), 0);
        assert_eq!(orch.turn_index, 0, "turn_index must not increment on error");
        assert_eq!(
            sink.snapshot().len(),
            1,
            "incomplete turn is still recorded"
        );
        assert!(
            !record.turn_errors.is_empty(),
            "turn_errors should capture the failure"
        );
    }

    #[tokio::test]
    async fn full_mode_records_english_control_run() {
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("hi", "你好");
        translator.add_zh_to_en("你好。", "Hi.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple(
            "你好。",
            Usage {
                input_tokens: 4,
                output_tokens: 5,
                cache_read: Some(100),
                cache_write: Some(50),
            },
        );
        backend.enqueue_simple(
            "Hi.",
            Usage {
                input_tokens: 6,
                output_tokens: 8,
                cache_read: Some(200),
                cache_write: Some(40),
            },
        );

        let sink = Arc::new(MemorySink::new());
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::Api,
            claude_model: "claude-sonnet-4-6".into(),
            translator_model: "qwen3:14b".into(),
            control_mode: ControlMode::Full,
        };
        let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
        let mut orch = Orchestrator::new(cfg, translator, backend, tokenizer, sink.clone());
        let mut out = CollectSink::default();
        let record = orch.run_turn("hi", &mut out).await.unwrap();

        let control = record.english_control_run.expect("control run captured");
        assert_eq!(control.prompt_tokens_reported, 6);
        assert_eq!(control.response_tokens_reported, 8);
        assert_eq!(control.cache_read_tokens_reported, Some(200));
        assert_eq!(control.cache_write_tokens_reported, Some(40));
    }
}
