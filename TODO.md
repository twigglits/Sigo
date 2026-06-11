# Sigo Improvement To-Do List

_All items completed or explicitly cancelled. Historical record of architectural decisions._

## 🟢 Usability
- [x] **REPL History**: Persistent history via rustyline's `history` file — working.
- [x] **TTFT Spinner**: Added indicatif spinner while waiting for first token.
- [x] **Config Pre-flight**: `sigo doctor` validates `sigo.toml`.
- [x] **Slash-Commands**: `/clear`, `/help`, `/quit`/`/exit`, `/verbose`, `/reset`, `/control-mode`, `/model`, `/backend`, `/bench`.

## 🟡 Abstraction
- [x] **RPITIT (enum dispatch)**: Native `async fn` traits via `AnyTranslator`/`AnyClaudeBackend` enums. Replaces `Arc<dyn Trait>` pattern — hot-swap preserved by field assignment on the enum.
- [ ] ~~**Declarative Config**~~: Cancelled — `merge_into` + serde + env overlay is clean enough; `config-rs` not worth the complexity.
- [x] **Backend Traits**: `ClaudeConfig` has `temperature`/`top_p`; `ApiBackend::with_options`; env vars `SIGO_CLAUDE_TEMPERATURE`/`SIGO_CLAUDE_TOP_P`.

## 🔴 Security
- [x] **Cross-platform Sandbox**: In-process Python preamble nulls socket/urllib/ctypes/ffi/asyncio; macOS `sandbox-exec` fallback.
- [x] **Reqwest Timeouts**: Explicit `timeout()` on every API call.
- [x] **Input Sanitization**: `translator/sanitize.rs` strips null/control chars, neuters `<source>` markers.

## 🔵 Reliability
- [x] **Transient Error Retries**: `tokio-retry` on server errors (429, 5xx) with exponential backoff.
- [x] **Graceful Shutdown**: `BenchmarkSink::flush()` + `JsonlSink::Drop` flushes on Ctrl-C.
- [x] **Ollama Health Check**: Heartbeat before each turn.

## 🟣 Modern Best Practices
- [x] **Structured Tracing**: `#[instrument]` on `run_turn` + `info_span!` for phases.
- [x] **Dependency Audit**: All deps actively used. Duplicate warnings noted (upstream transitive).
- [x] **CI Token Regression**: Snapshot-based test for deterministic token counts through sanitize→compact→count pipeline.
