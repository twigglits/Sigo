# Sigo Improvement To-Do List

## 🟢 Usability
- [ ] **REPL History**: Verify/Implement persistent history saving for `rustyline`.
- [ ] **TTFT Spinner**: Add a visual spinner (e.g., `indicatif`) while waiting for the first token (TTFT).
- [ ] **Config Pre-flight**: Add a validation step for `sigo.toml` in `sigo doctor`.
- [ ] **More Slash-Commands**: Add `/clear` to purge the current session without `/reset`.

## 🟡 Abstraction
- [ ] **RPITIT**: Migrate from `async-trait` to native async traits (Rust 1.88+).
- [ ] **Declarative Config**: Explore replacing manual `merge_into` with `config-rs` or a more robust layering pattern.
- [ ] **Backend Traits**: Refine `ClaudeBackend` and `Translator` traits to support more configurations (e.g., temperature, top_p).

## 🔴 Security
- [ ] **Cross-platform Sandbox**: Implement a lighter sandbox for non-Linux (macOS/Windows) instead of relying solely on `bubblewrap`.
- [ ] **Reqwest Timeouts**: Audit all API calls to ensure explicit `timeout()` is applied per request.
- [ ] **Input Sanitization**: Ensure prompts are sanitized before being passed to the local translator to prevent "prompt injection" into the translator itself.

## 🔵 Reliability
- [ ] **Transient Error Retries**: Implement a retry mechanism for `reqwest` calls (e.g., using `tokio-retry`).
- [ ] **Graceful Shutdown**: Ensure `BenchmarkSink` is flushed correctly on `Ctrl-C`.
- [ ] **Ollama Health Check**: Integrate a heartbeat check in the orchestrator before starting a turn.

## 🟣 Modern Best Practices
- [ ] **Structured Tracing**: Implement `tracing::span` in `orchestrator::run_turn` for better observability.
- [ ] **Dependency Audit**: Review `Cargo.toml` for outdated crates or redundant dependencies.
- [ ] **CI Integration**: Add a "dry-run" benchmark in CI to detect token regressions.
