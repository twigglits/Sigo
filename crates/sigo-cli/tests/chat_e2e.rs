use std::sync::Arc;

use sigo_cli::commands::chat;
use sigo_core::{
    BackendKind, CollectSink, ControlMode, FakeBackend, FakeTranslator, MemorySink, Orchestrator,
    OrchestratorConfig, Tokenizer, TokenizerProxy, Usage,
};

#[tokio::test]
async fn chat_run_once_emits_translated_answer_and_trailing_newline() {
    let translator = Arc::new(FakeTranslator::new());
    translator.add_en_to_zh("Hello, world!", "你好，世界！");
    translator.add_zh_to_en("你好，世界！", "Hello, world!");

    let backend = Arc::new(FakeBackend::new());
    backend.enqueue_simple(
        "你好，世界！",
        Usage { input_tokens: 5, output_tokens: 5, ..Default::default() },
    );

    let sink = Arc::new(MemorySink::new());
    let tokenizer: Arc<dyn Tokenizer> = Arc::new(TokenizerProxy::new().unwrap());
    let cfg = OrchestratorConfig {
        backend_kind: BackendKind::Api,
        claude_model: "claude-sonnet-4-6".into(),
        translator_model: "fake".into(),
        control_mode: ControlMode::PromptOnly,
    };
    let mut orch = Orchestrator::new(cfg, translator, backend, tokenizer, sink.clone());

    let mut out = CollectSink::default();
    chat::run_once(&mut orch, "Hello, world!", &mut out, false)
        .await
        .expect("turn should complete");

    assert!(out.buf.contains("Hello, world!"), "answer streamed to sink: {:?}", out.buf);
    assert!(out.buf.ends_with('\n'), "trailing newline appended");
    assert_eq!(sink.snapshot().len(), 1, "turn recorded once");
}
