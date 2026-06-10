use chrono::Utc;
use futures::stream::FuturesOrdered;
use futures::StreamExt;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
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
    /// Running local-token totals of the committed conversation history, kept so the
    /// per-turn cumulative counts are O(1) instead of re-tokenizing the whole history.
    chinese_convo_tokens: u32,
    english_convo_tokens: u32,
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
            chinese_convo_tokens: 0,
            english_convo_tokens: 0,
        }
    }

    pub fn reset(&mut self) {
        self.session_id = Uuid::new_v4();
        self.turn_index = 0;
        self.chinese_convo = Conversation::new();
        self.english_convo = Conversation::new();
        self.chinese_convo_tokens = 0;
        self.english_convo_tokens = 0;
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
        let raw_chinese_prompt = self
            .translator
            .translate(english_prompt, Direction::EnToZh)
            .await?;
        let translation_in_ms = translation_in_started.elapsed().as_millis() as u64;

        // Step 2: Local token counts, and whitespace compaction of the outbound
        // prompt guarded by a live comparison: BPE merges are nonlinear, so
        // "compaction never costs tokens" is enforced by measurement here, not
        // assumed. Everything downstream (send, record, history, counts) sees
        // only the winning form; the pre-compaction count is recorded so the
        // delta stays attributable from bench artifacts.
        let english_prompt_tokens_local = self
            .tokenizer
            .count_tokens(english_prompt)
            .unwrap_or_else(|e| {
                errors.push(format!("tokenizer en prompt: {e}"));
                0
            });
        let chinese_prompt_tokens_precompact_local = self
            .tokenizer
            .count_tokens(&raw_chinese_prompt)
            .unwrap_or_else(|e| {
                errors.push(format!("tokenizer zh prompt (precompact): {e}"));
                0
            });
        let compacted = crate::compact::compact_zh(&raw_chinese_prompt);
        let compacted_tokens = self.tokenizer.count_tokens(&compacted).unwrap_or_else(|e| {
            errors.push(format!("tokenizer zh prompt: {e}"));
            0
        });
        let (chinese_prompt, chinese_prompt_tokens_local) =
            if compacted_tokens <= chinese_prompt_tokens_precompact_local {
                (compacted, compacted_tokens)
            } else {
                (raw_chinese_prompt, chinese_prompt_tokens_precompact_local)
            };

        // Cumulative totals: committed history (running counter) + this prompt.
        let chinese_cumulative_prompt_tokens_local =
            self.chinese_convo_tokens + chinese_prompt_tokens_local;
        let english_cumulative_prompt_tokens_local =
            self.english_convo_tokens + english_prompt_tokens_local;

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

        // Step 4: Drain the Claude stream while translating completed sentences
        // concurrently (bounded), emitting results in production order. Decoupling
        // translation from stream reading means a long answer no longer pays N
        // sequential translation latencies.
        const MAX_INFLIGHT: usize = 4;
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
        let mut translation_out_calls: u32 = 0;
        let mut tx_span_min: Option<Instant> = None;
        let mut tx_span_max: Option<Instant> = None;
        let mut incomplete = false;

        // Segments awaiting a translation slot (FIFO, so output stays in order) and the
        // in-flight translations (FuturesOrdered yields completed work front-first).
        let mut queue: VecDeque<Segment> = VecDeque::new();
        let mut pending: FuturesOrdered<Pin<Box<dyn Future<Output = Emit> + Send>>> =
            FuturesOrdered::new();
        let mut stream_finished = false;

        loop {
            // Promote queued segments into in-flight translations up to the cap.
            while pending.len() < MAX_INFLIGHT {
                match queue.pop_front() {
                    Some(seg) => pending.push_back(translate_segment(self.translator.clone(), seg)),
                    None => break,
                }
            }

            if pending.is_empty() && queue.is_empty() && stream_finished {
                break;
            }

            if stream_finished {
                // No more chunks: drain remaining in-flight translations in order.
                if let Some(emit) = pending.next().await {
                    apply_emit(
                        emit,
                        out,
                        &mut english_response_emitted,
                        &mut translation_out_calls,
                        &mut tx_span_min,
                        &mut tx_span_max,
                        &mut errors,
                    );
                }
                continue;
            }

            // Read the next chunk while completed translations drain concurrently.
            tokio::select! {
                biased;
                Some(emit) = pending.next(), if !pending.is_empty() => {
                    apply_emit(
                        emit,
                        out,
                        &mut english_response_emitted,
                        &mut translation_out_calls,
                        &mut tx_span_min,
                        &mut tx_span_max,
                        &mut errors,
                    );
                }
                item = stream.next() => {
                    match item {
                        None => {
                            queue.extend(buffer.flush());
                            stream_finished = true;
                        }
                        Some(Ok(ResponseChunk::TextDelta(text))) => {
                            if !got_first_delta {
                                ttft_ms = claude_started.elapsed().as_millis() as u64;
                                got_first_delta = true;
                            }
                            chinese_response.push_str(&text);
                            queue.extend(buffer.push(&text));
                        }
                        Some(Ok(ResponseChunk::Done { usage, stop_reason: _ })) => {
                            reported_input = Some(usage.input_tokens);
                            reported_output = Some(usage.output_tokens);
                            cache_read = usage.cache_read;
                            cache_write = usage.cache_write;
                            queue.extend(buffer.flush());
                            stream_finished = true;
                        }
                        Some(Err(e)) => {
                            queue.extend(buffer.flush());
                            incomplete = true;
                            errors.push(format!("claude stream: {e}"));
                            stream_finished = true;
                        }
                    }
                }
            }
        }
        // Wall-clock span during which translation work happened (accounts for overlap),
        // not the sum of per-call durations (which would over-count under concurrency).
        let translation_out_ms_total = match (tx_span_min, tx_span_max) {
            (Some(s), Some(e)) => e.saturating_duration_since(s).as_millis() as u64,
            _ => 0,
        };
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
            // Advance the running token totals by exactly this turn's contribution
            // (prompt + response on each side), matching a full re-tokenization.
            let english_response_tokens_local = self
                .tokenizer
                .count_tokens(&english_response_emitted)
                .unwrap_or(0);
            self.chinese_convo_tokens +=
                chinese_prompt_tokens_local + chinese_response_tokens_local;
            self.english_convo_tokens +=
                english_prompt_tokens_local + english_response_tokens_local;
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
            chinese_prompt_tokens_precompact_local,
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

/// One ready-to-write output fragment for a streamed segment. Passthrough fragments
/// carry no timing; translated fragments carry the (start, end) of their translation so
/// the orchestrator can report the wall-clock span of overlapping translation work.
struct Emit {
    text: String,
    timing: Option<(Instant, Instant)>,
    error: Option<String>,
}

/// Build the future that produces a segment's output. Passthrough is ready immediately;
/// text is translated ZH->EN, falling back to the raw ZH on error. The translator handle
/// is cloned so the future is `'static` and can run concurrently with stream draining.
fn translate_segment(
    translator: Arc<dyn Translator>,
    seg: Segment,
) -> Pin<Box<dyn Future<Output = Emit> + Send>> {
    match seg {
        Segment::Passthrough(raw) => Box::pin(async move {
            Emit {
                text: raw,
                timing: None,
                error: None,
            }
        }),
        Segment::Text(zh) => Box::pin(async move {
            let start = Instant::now();
            let result = translator.translate(&zh, Direction::ZhToEn).await;
            let end = Instant::now();
            match result {
                Ok(en) => Emit {
                    text: en,
                    timing: Some((start, end)),
                    error: None,
                },
                Err(e) => Emit {
                    text: zh,
                    timing: Some((start, end)),
                    error: Some(format!("zh->en segment translation: {e}")),
                },
            }
        }),
    }
}

/// Write one emitted fragment in order, updating the translation call count, the
/// wall-clock translation span, and any error.
#[allow(clippy::too_many_arguments)]
fn apply_emit(
    emit: Emit,
    out: &mut dyn OutputSink,
    english_acc: &mut String,
    calls: &mut u32,
    span_min: &mut Option<Instant>,
    span_max: &mut Option<Instant>,
    errors: &mut Vec<String>,
) {
    out.write(&emit.text);
    english_acc.push_str(&emit.text);
    if let Some((s, e)) = emit.timing {
        *calls += 1;
        *span_min = Some(span_min.map_or(s, |m| m.min(s)));
        *span_max = Some(span_max.map_or(e, |m| m.max(e)));
    }
    if let Some(err) = emit.error {
        errors.push(err);
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
    async fn cumulative_local_counts_equal_full_history_retokenization() {
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("hi", "你好");
        translator.add_zh_to_en("你好。", "Hi.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple("你好。", Usage::default());
        backend.enqueue_simple("你好。", Usage::default());
        let sink = Arc::new(MemorySink::new());
        let mut orch = build(translator, backend, sink);
        let mut out = CollectSink::default();
        let r1 = orch.run_turn("hi", &mut out).await.unwrap();
        let r2 = orch.run_turn("hi", &mut out).await.unwrap();

        let tk = TokenizerProxy::new().unwrap();
        let t = |s: &str| tk.count_tokens(s).unwrap();
        // Turn 0: empty history + this prompt.
        assert_eq!(r1.chinese_cumulative_prompt_tokens_local, t("你好"));
        assert_eq!(r1.english_cumulative_prompt_tokens_local, t("hi"));
        // Turn 1: prior [prompt, response] pair + this prompt — exactly the sum a
        // from-scratch re-tokenization of the whole history would produce.
        assert_eq!(
            r2.chinese_cumulative_prompt_tokens_local,
            t("你好") + t("你好。") + t("你好")
        );
        assert_eq!(
            r2.english_cumulative_prompt_tokens_local,
            t("hi") + t("Hi.") + t("hi")
        );
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
    async fn translations_overlap_and_preserve_order() {
        use std::time::Duration;

        // A translator that sleeps a fixed delay per call so we can observe overlap.
        struct Sleepy {
            d: Duration,
        }
        #[async_trait::async_trait]
        impl Translator for Sleepy {
            async fn translate(&self, text: &str, dir: Direction) -> Result<String> {
                tokio::time::sleep(self.d).await;
                Ok(match dir {
                    Direction::EnToZh => text.to_string(),
                    Direction::ZhToEn => format!("[{}]", text.trim_end_matches('。').trim()),
                })
            }
        }

        let translator: Arc<dyn Translator> = Arc::new(Sleepy {
            d: Duration::from_millis(100),
        });
        let backend = Arc::new(FakeBackend::new());
        // One delta with four complete Chinese sentences → four Text segments.
        backend.enqueue_simple("句一。句二。句三。句四。", Usage::default());
        let sink = Arc::new(MemorySink::new());
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::Api,
            claude_model: "m".into(),
            translator_model: "t".into(),
            control_mode: ControlMode::PromptOnly,
        };
        let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
        let mut orch = Orchestrator::new(cfg, translator, backend, tokenizer, sink);

        let mut out = CollectSink::default();
        let start = std::time::Instant::now();
        let record = orch.run_turn("hello", &mut out).await.unwrap();
        let elapsed = start.elapsed();

        // Output order matches production order.
        let o = &record.english_response;
        let (p1, p2, p3, p4) = (
            o.find("句一").unwrap(),
            o.find("句二").unwrap(),
            o.find("句三").unwrap(),
            o.find("句四").unwrap(),
        );
        assert!(p1 < p2 && p2 < p3 && p3 < p4, "segments out of order: {o}");
        assert_eq!(record.translation_out_calls, 4);

        // Sequential would be ~500ms (1 prompt + 4 sentence translations at 100ms each);
        // overlapping the sentence translations brings it well under that.
        assert!(
            elapsed < Duration::from_millis(375),
            "translations did not overlap: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn run_turn_sends_and_records_compacted_zh() {
        let en = "Use tokio in Rust for concurrency.";
        let raw_zh = "在 Rust 中使用 tokio 实现并发。";
        let compacted = "在Rust中使用tokio实现并发。";
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh(en, raw_zh);
        translator.add_zh_to_en("好。", "OK.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple("好。", Usage::default());
        let sink = Arc::new(MemorySink::new());
        let mut orch = build(translator, backend.clone(), sink);

        let mut out = CollectSink::default();
        let record = orch.run_turn(en, &mut out).await.unwrap();

        // The compacted form is what is sent, recorded, counted, and committed
        // to the replayed history; the pre-compaction count is kept alongside
        // so savings stay attributable from bench artifacts.
        assert_eq!(backend.sent_prompts(), vec![compacted.to_string()]);
        assert_eq!(record.chinese_prompt, compacted);
        assert_eq!(orch.chinese_convo.messages[0].content, compacted);
        let tk = TokenizerProxy::new().unwrap();
        assert_eq!(
            record.chinese_prompt_tokens_local,
            tk.count_tokens(compacted).unwrap()
        );
        assert_eq!(
            record.chinese_prompt_tokens_precompact_local,
            tk.count_tokens(raw_zh).unwrap()
        );
    }

    /// Tokenizer stub under which the (shorter) compacted text counts MORE
    /// tokens, inverting the guard's comparison.
    struct InvertedTokenizer;
    impl Tokenizer for InvertedTokenizer {
        fn count_tokens(&self, text: &str) -> Result<u32> {
            Ok(10_000u32.saturating_sub(text.chars().count() as u32))
        }
    }

    #[tokio::test]
    async fn compaction_guard_falls_back_to_raw_when_not_cheaper() {
        let en = "Use tokio in Rust.";
        let raw_zh = "在 Rust 中使用 tokio。";
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh(en, raw_zh);
        translator.add_zh_to_en("好。", "OK.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple("好。", Usage::default());
        let cfg = OrchestratorConfig {
            backend_kind: BackendKind::Api,
            claude_model: "m".into(),
            translator_model: "t".into(),
            control_mode: ControlMode::PromptOnly,
        };
        let mut orch = Orchestrator::new(
            cfg,
            translator,
            backend.clone(),
            Arc::new(InvertedTokenizer),
            Arc::new(MemorySink::new()),
        );

        let mut out = CollectSink::default();
        let record = orch.run_turn(en, &mut out).await.unwrap();

        // Under this tokenizer the compacted candidate is "more expensive", so
        // the raw translation must be sent and recorded.
        assert_eq!(backend.sent_prompts(), vec![raw_zh.to_string()]);
        assert_eq!(record.chinese_prompt, raw_zh);
        assert_eq!(orch.chinese_convo.messages[0].content, raw_zh);
    }

    #[tokio::test]
    async fn assistant_history_is_raw_streamed_response() {
        // The response contains compactable CJK-Latin spacing; replayed history
        // must keep Claude's bytes untouched (record honesty + prompt caching).
        let zh_resp = "用 Rust 写的。";
        let translator = Arc::new(FakeTranslator::new());
        translator.add_en_to_zh("hi", "你好");
        translator.add_zh_to_en(zh_resp, "Written in Rust.");
        let backend = Arc::new(FakeBackend::new());
        backend.enqueue_simple(zh_resp, Usage::default());
        let sink = Arc::new(MemorySink::new());
        let mut orch = build(translator, backend, sink);

        let mut out = CollectSink::default();
        let record = orch.run_turn("hi", &mut out).await.unwrap();

        assert_eq!(record.chinese_response, zh_resp);
        assert_eq!(orch.chinese_convo.messages[1].content, zh_resp);
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
