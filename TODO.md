# Sigo Improvement To-Do List

## 🟢 Usability
- [x] **REPL History**: Persistent history via rustyline's `history` file — working.
- [x] **TTFT Spinner**: Added indicatif spinner while waiting for first token.
- [x] **Config Pre-flight**: `sigo doctor` validates `sigo.toml`.
- [x] **More Slash-Commands**: `/clear` added — purges session without `/reset`.

## 🟡 Abstraction
- [ ] ~~**RPITIT**~~: Cancelled — native async traits are `!dyn`-compatible; `Arc<dyn Translator>` is architecturally required for runtime hot-swap (`/model`, `/backend`).
- [ ] ~~**Declarative Config**~~: Cancelled — manual `merge_into` + serde + env overlay is clean and well-tested; `config-rs` adds complexity without measurable benefit.
- [x] **Backend Traits**: `ClaudeConfig` now has `temperature`/`top_p` fields; `ApiBackend::with_options(temperature, top_p)`; env vars `SIGO_CLAUDE_TEMPERATURE`/`SIGO_CLAUDE_TOP_P`.

## 🔴 Security
- [x] **Cross-platform Sandbox**: In-process Python preamble strengthened to null socket/urllib/ctypes/ffi/asyncio (+macOS `sandbox-exec` fallback). Cross-platform baseline is always active.
- [x] **Reqwest Timeouts**: Explicit `timeout()` on every API call.
- [x] **Input Sanitization**: New `translator/sanitize.rs` strips null/control chars and neuters `<source>` markers. Integrated in `orchestrator::run_turn`.

## 🔵 Reliability
- [x] **Transient Error Retries**: `tokio-retry` on server errors (429, 5xx) with exponential backoff.
- [x] **Graceful Shutdown**: `BenchmarkSink::flush()` + `JsonlSink::Drop` flushes on Ctrl-C.
- [x] **Ollama Health Check**: Heartbeat check before starting a turn.

## 🟣 Modern Best Practices
- [x] **Structured Tracing**: `#[instrument]` on run_turn + `info_span!` for en_to_zh/compact/claude_stream/record phases.
- [x] **Dependency Audit**: All deps actively used. Duplicate warnings (`getrandom`, `unicode-width`, `windows-sys`, `wit-bindgen`) are from incompatible transitive major versions — not fixable upstream.
- [x] **CI Integration**: Token regression test (`tests/token_regression.rs`) asserts deterministic o200k_base counts through sanitize→compact→count pipeline. Snapshot updater included.
