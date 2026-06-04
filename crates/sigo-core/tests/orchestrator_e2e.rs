use sigo_core::{
    BackendKind, CollectSink, ControlMode, FakeBackend, FakeTranslator, MemorySink, Orchestrator,
    OrchestratorConfig, ResponseChunk, Tokenizer, TokenizerProxy, Usage,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn multi_turn_session_advances_history_and_records_each_turn() {
    let translator = Arc::new(FakeTranslator::new());
    translator.add_en_to_zh("ping", "乒");
    translator.add_zh_to_en("乓。", "Pong.");
    translator.add_en_to_zh("again", "再");
    translator.add_zh_to_en("再乓。", "Pong again.");

    let backend = Arc::new(FakeBackend::new());
    backend.enqueue_simple(
        "乓。",
        Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..Default::default()
        },
    );
    backend.enqueue_simple(
        "再乓。",
        Usage {
            input_tokens: 5,
            output_tokens: 1,
            ..Default::default()
        },
    );

    let sink = Arc::new(MemorySink::new());

    let cfg = OrchestratorConfig {
        backend_kind: BackendKind::Api,
        claude_model: "claude-sonnet-4-6".into(),
        translator_model: "fake".into(),
        control_mode: ControlMode::PromptOnly,
    };
    let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
    let mut orch = Orchestrator::new(cfg, translator, backend, tokenizer, sink.clone());

    let mut out1 = CollectSink::default();
    let r1 = orch.run_turn("ping", &mut out1).await.unwrap();
    assert!(!r1.incomplete);
    assert!(out1.buf.contains("Pong"));

    let mut out2 = CollectSink::default();
    let r2 = orch.run_turn("again", &mut out2).await.unwrap();
    assert!(!r2.incomplete);
    assert!(out2.buf.contains("Pong again"));

    assert_eq!(orch.chinese_convo.messages.len(), 4);
    assert_eq!(sink.snapshot().len(), 2);
    assert!(r2.chinese_cumulative_prompt_tokens_local > r1.chinese_cumulative_prompt_tokens_local);
}

#[tokio::test]
async fn stream_without_done_still_records_a_turn() {
    let translator = Arc::new(FakeTranslator::new());
    translator.add_en_to_zh("ping", "乒");
    translator.add_zh_to_en("乓", "Pong");

    let backend = Arc::new(FakeBackend::new());
    // Script: yield a text delta but NO Done event. The stream ends cleanly afterward.
    let scripted: Vec<(ResponseChunk, Duration)> = vec![(
        ResponseChunk::TextDelta("乓".into()),
        Duration::from_millis(0),
    )];
    backend.enqueue_turn(scripted);

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

    // Stream ended without Done — orchestrator treats this as clean completion since no Err arrived.
    // History advances; record is appended.
    assert_eq!(sink.snapshot().len(), 1);
    assert!(record.chinese_response.contains("乓"));
}
