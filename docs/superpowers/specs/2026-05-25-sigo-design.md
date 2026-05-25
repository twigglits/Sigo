# Sigo — Design Spec

**Date:** 2026-05-25
**Status:** Draft, awaiting user review

## 1. Purpose

Sigo is a Rust CLI that routes user prompts through a local LLM (Ollama-hosted Gemma 3 / Qwen) which translates English to Chinese, sends the Chinese prompt to Claude, then translates Claude's Chinese response back to English for the user. Every turn is recorded so the user can answer one concrete research question: **do Chinese prompts use fewer Claude tokens than the equivalent English prompts?**

The translation layer is not the product; the benchmark is the product. The CLI exists to make collecting comparable per-turn token data effortless.

## 2. Scope

### In scope (v1)

- CLI REPL with line editing and history.
- Two selectable Claude backends: Anthropic Messages API and the local `claude` CLI (Claude Code), chosen by config or flag.
- One translator provider: Ollama, with model selectable at runtime (Qwen, Gemma 3, etc.).
- Sentence-buffered streaming so Claude's Chinese output is translated to English in coherent chunks as it streams.
- Per-turn benchmark records appended to a single JSONL log.
- Local tokenizer (bundled Anthropic Claude 2 tokenizer JSON) for English-control token counts.
- `sigo bench` subcommands for summary, single-record inspection, and CSV/JSONL export.
- `sigo doctor` for first-run setup checks.

### Out of scope (deliberate YAGNI)

No web UI or TUI dashboard. No daemon / IPC / multi-client. No translator providers other than Ollama. No translation caching. No glossary or terminology file. No automatic Ollama lifecycle management. No framework-level tool/file operations (Claude Code backend handles its own tools and we pass their output through untranslated). No conversation branching, multi-thread sessions, or history editing. No notifications, schedulers, or hooks. No automated benchmark plotting (`bench export` produces CSV/JSONL; plotting lives in the user's notebooks). No retry-after-edit of past turns — `/reset` is the only recovery.

## 3. High-level architecture

Two-crate Cargo workspace:

- **`crates/sigo-core`** — library. All trait definitions, orchestration, sentence-buffer stream transformer, conversation and benchmark types, and concrete adapter implementations. Library code only touches I/O via the adapter implementations; the orchestration layer is I/O-free.
- **`crates/sigo-cli`** — binary. `clap` argument parsing, TOML config loading, `rustyline` REPL, terminal rendering. Wires concrete adapters into the orchestrator. Knows nothing about HTTP or SSE.

This split exists so the orchestration loop is testable with fakes and so a future non-TTY driver (e.g., a benchmark harness) can drive turns without scraping a terminal.

### Runtime dependencies

- `tokio` — async runtime.
- `reqwest` — HTTP client with streaming bodies.
- `eventsource-stream` — SSE parsing for the Anthropic API backend.
- `serde` + `serde_json` + `toml` — config, JSONL records, NDJSON parsing.
- `clap` — CLI argument parsing.
- `rustyline` — REPL line editing and history.
- `tokenizers` (HuggingFace) — loads the bundled Claude 2 tokenizer JSON.
- `tracing` + `tracing-subscriber` — structured logging.
- `thiserror` (lib) + `anyhow` (bin) — error types.
- `uuid` — session IDs.
- `chrono` — timestamps.
- `insta` — snapshot testing in dev-dependencies.

## 4. Core abstractions

Four traits in `sigo-core` define every external dependency. The orchestrator depends only on these.

```rust
#[async_trait]
pub trait Translator: Send + Sync {
    async fn translate(&self, text: &str, dir: Direction) -> Result<String>;
}
pub enum Direction { EnToZh, ZhToEn }

#[async_trait]
pub trait ClaudeBackend: Send + Sync {
    async fn stream_turn(
        &self,
        convo: &Conversation,
        prompt: &str,
    ) -> Result<BoxStream<'static, Result<ResponseChunk>>>;
}

pub enum ResponseChunk {
    TextDelta(String),
    Done { usage: Usage },
}

pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read: Option<u32>,
    pub cache_write: Option<u32>,
}

pub trait Tokenizer: Send + Sync {
    fn count_tokens(&self, text: &str) -> Result<u32>;
}

pub trait BenchmarkSink: Send + Sync {
    fn record(&self, turn: &TurnRecord) -> Result<()>;
}
```

### Concrete adapter implementations

- **`OllamaTranslator`** — non-streaming POST to `/api/chat`. We want the full translated text before forwarding because the translator is being fed a coherent unit (one prompt, or one buffered sentence). The translator's own internal streaming is irrelevant.
- **`ApiBackend`** — Anthropic Messages API with `stream=true`. Parses SSE events (`message_start`, `content_block_delta`, `message_delta`, `message_stop`) into `ResponseChunk`s. Final `message_delta` carries `usage`.
- **`ClaudeCodeBackend`** — spawns `claude -p <prompt> --output-format stream-json --input-format stream-json` as a child process. Parses NDJSON events from stdout. Resumes via `--resume <session-id>` for multi-turn. Tool-result events emit as `TextDelta`s that the sentence buffer routes through its passthrough branch.
- **`ClaudeTokenizer`** — bundles `assets/claude2-tokenizer.json` (~1 MB), loaded once via `tokenizers` crate. Pure CPU.
- **`JsonlSink`** — `BufWriter` over an append-mode file handle, flush after every record so a crash loses at most the in-flight turn.

## 5. Streaming pipeline

A single per-turn orchestration task runs the following sequence:

1. User types English; presses Enter.
2. **EN → ZH** via `Translator::translate`. Await the full result. Record `translation_in_ms`.
3. Append the ZH message to `chinese_convo` and the EN message to `english_convo` (parallel transcripts).
4. **Local tokenization snapshot** for both prompts (both messages individually and the cumulative session totals).
5. Open the Claude stream with `chinese_convo`. Backend returns a `Stream<ResponseChunk>`. Note `claude_ttft_ms` on first `TextDelta`.
6. For each `TextDelta`:
   - Append to `chinese_response` (full record).
   - Feed into the sentence-buffer state machine.
   - On each completed segment yielded by the buffer:
     - **Text segment** → `Translator::translate(ZhToEn)` → write English to stdout. Append the English to a `english_response_emitted` accumulator.
     - **Code block / passthrough** → write raw to stdout. Append raw to `english_response_emitted` unchanged.
   - Resume pulling chunks. The serial structure preserves output ordering; SSE/NDJSON handle backpressure naturally.
7. On `ResponseChunk::Done`, flush any trailing buffered segment, capture authoritative `usage`.
8. Append `chinese_response` to `chinese_convo`. Append `english_response_emitted` to `english_convo`.
9. Compute timing fields; write `TurnRecord` to `BenchmarkSink`.
10. (If `control_mode = "full"`) the English control Claude run is launched as a concurrent `tokio` task at step 5 alongside the Chinese stream; the TurnRecord write at step 9 awaits both. The user does wait longer on `full` mode — that is the explicit trade-off for the strongest direct comparison. If the control run fails, `english_control_run` stays `None` and the failure is appended to `turn_errors`.

### Sentence-buffer state machine

Two states. Lives in `crates/sigo-core/src/stream/sentence_buffer.rs`.

- **`Text`** — accumulates chars. Yields a `Segment::Text` on:
  - sentence terminator `。`, `！`, `？`, `!`, `?` followed by whitespace or end-of-stream;
  - paragraph break `\n\n`;
  - transition into a code fence (yields any pending text first, then transitions).
- **`CodeFence`** — accumulates chars verbatim. Yields a `Segment::Passthrough` on closing ``` ```.

Inline code (single backticks) stays inside text segments; the translator's preservation system prompt handles those.

The recorded English transcript is exactly what was emitted to the user — no separate "clean retranslation" pass. This keeps the JSONL faithful to lived experience and avoids double translation cost.

## 6. Benchmark instrumentation

### `TurnRecord` schema (JSONL, schema-versioned)

```rust
pub struct TurnRecord {
    pub schema_version: u32,          // start at 1
    pub session_id: Uuid,
    pub turn_index: u32,
    pub timestamp: DateTime<Utc>,
    pub backend: BackendKind,         // "api" | "claude-code"
    pub claude_model: String,
    pub translator_model: String,

    pub english_prompt: String,
    pub chinese_prompt: String,
    pub chinese_response: String,
    pub english_response: String,     // exactly what the user saw

    pub english_prompt_tokens_local: u32,
    pub chinese_prompt_tokens_local: u32,
    pub chinese_response_tokens_local: u32,

    pub chinese_prompt_tokens_reported: Option<u32>,
    pub chinese_response_tokens_reported: Option<u32>,
    pub cache_read_tokens_reported: Option<u32>,
    pub cache_write_tokens_reported: Option<u32>,

    pub chinese_cumulative_prompt_tokens_local: u32,
    pub english_cumulative_prompt_tokens_local: u32,

    pub english_control_run: Option<EnglishControlRun>,

    pub incomplete: bool,             // true if the Chinese stream disconnected mid-turn
    pub turn_errors: Vec<String>,     // non-fatal errors logged during the turn (tokenizer, control run, etc.)

    pub translation_in_ms: u64,
    pub translation_out_ms_total: u64,
    pub translation_out_calls: u32,
    pub claude_ttft_ms: u64,
    pub claude_total_ms: u64,
    pub turn_total_ms: u64,
}

pub struct EnglishControlRun {
    pub english_response: String,
    pub prompt_tokens_reported: u32,
    pub response_tokens_reported: u32,
    pub duration_ms: u64,
}
```

### Storage

Single rolling file at `$XDG_DATA_HOME/sigo/turns.jsonl`. Sessions are distinguished by `session_id`. `BufWriter` flushes per record.

### Calibration

The local tokenizer is an approximation; the Claude 2 tokenizer is in the same BPE family as later Claude models but not identical. For every Chinese turn we hold both the local count and Claude's authoritative count, so the ratio `chinese_prompt_tokens_local / chinese_prompt_tokens_reported` is a per-turn calibration factor. `sigo bench summary` reports the rolling mean and uses it to convert English-local counts into estimated authoritative-token numbers for the savings percentage.

### `sigo bench` subcommands

- `sigo bench summary [--session <id>] [--last N]` — turn count, mean/median per turn for {EN-prompt local, ZH-prompt local, ZH-prompt reported, ZH-response reported}, cumulative session totals, calibration factor, estimated savings percentage (Chinese reported vs estimated English authoritative).
- `sigo bench show <session-id> <turn-index>` — dump the full record.
- `sigo bench export --format csv|jsonl [--session <id>]` — flat export for notebook analysis.

### Control modes

- **`off`** — no control measurement; only Chinese-side records.
- **`prompt-only`** *(default)* — local tokenization of the English transcript each turn; near-free.
- **`full`** — fire a parallel English Claude run per turn and capture its authoritative usage. Doubles API cost; enables the strongest direct comparison.

## 7. Configuration & CLI

### Config file

Resolution order (later sources override earlier): built-in defaults → `$XDG_CONFIG_HOME/sigo/config.toml` → `./sigo.toml` → CLI flags → REPL slash-commands (runtime, not persisted).

```toml
[translator]
provider = "ollama"
endpoint = "http://localhost:11434"
model = "qwen3:14b"
timeout_seconds = 60

[claude]
backend = "api"                    # or "claude-code"
model = "claude-sonnet-4-6"
max_tokens = 4096

[claude.claude_code]
binary = "claude"
extra_args = []

[benchmark]
log_path = "$XDG_DATA_HOME/sigo/turns.jsonl"
control_mode = "prompt-only"

[repl]
verbose = false
history_file = "$XDG_DATA_HOME/sigo/history"
```

Secrets stay out of the config file. `ANTHROPIC_API_KEY` comes from env only and is required only when `backend = "api"`. Claude Code backend delegates auth to the `claude` CLI.

### CLI

```
sigo                                            # start REPL
sigo --backend claude-code --verbose            # flags override config
sigo bench summary [--session <id>] [--last N]
sigo bench show <session-id> <turn-index>
sigo bench export --format csv|jsonl
sigo config show                                # print resolved effective config
sigo doctor                                     # connectivity & setup check
```

### `sigo doctor` checks

- Config file parses.
- Ollama reachable at the configured endpoint.
- Translator model present (via `/api/tags`); suggest `ollama pull <model>` if missing.
- Backend check:
  - API backend: `ANTHROPIC_API_KEY` set; ping with a tiny call to validate.
  - Claude Code backend: `claude --version` succeeds.
- Tokenizer JSON loads.
- Benchmark log path writable.

### REPL slash-commands

- `/help`, `/quit` (also Ctrl-D)
- `/verbose` — toggle rich display (ZH prompt + ZH response + token panel)
- `/reset` — clear conversation, start new `session_id`
- `/save <path>` — dump current session transcript
- `/model translator <name>` / `/model claude <name>` — hot-swap models
- `/backend <api|claude-code>` — hot-swap backend (warns about cache invalidation)
- `/control-mode <off|prompt-only|full>` — change for subsequent turns
- `/bench` — print current-session summary inline

### Display

Default (non-verbose): clean English response with a one-line footer per turn, e.g.

```
[turn 3 · 0.8s · -28% vs EN local-est]
```

Verbose: adds three panels above the footer — ZH prompt, ZH response, full token table.

## 8. Error handling

Four failure sources, each with a defined policy. The invariant across all of them: **failures never leave a half-committed turn. Either a turn is fully recorded and history advances, or it is discarded and history stays put.**

### Startup-time failures (fail fast, actionable messages)

- Config parse error → line/column + offending TOML excerpt.
- Ollama unreachable → `ollama serve` hint plus configured endpoint.
- Translator model missing → `ollama pull <model>` hint.
- `ANTHROPIC_API_KEY` missing for API backend → explain env var.
- `claude` binary missing for Claude Code backend → suggest install or path config.

These flow through `sigo doctor` so messages are written once.

### Per-turn translator failures

- Timeout (config-driven, default 60s) → REPL prompts `[r]etry / [s]kip / [c]ancel`. Conversation state unchanged on skip/cancel.
- HTTP 5xx / connection drop mid-stream → same retry UI.
- Empty or malformed response → append to `turn_errors`, present to user as timeout.

### Per-turn Claude failures

- Rate limit (HTTP 429) → exponential backoff with visible countdown, max 3 retries.
- Mid-stream disconnect → partial Chinese response preserved in the turn record with `incomplete = true`; partial English already on the user's screen stays; conversation history does NOT advance.
- Auth failures → bail with clear message, no retry.
- Claude Code child-process crash → capture stderr, surface to user, mark turn incomplete.

### Tokenizer / sink failures (non-fatal)

- Tokenizer error → log warning, set count fields to `0` and append to `turn_errors`, continue.
- Sink write failure → warn once, switch to in-memory buffer, retry on next turn. Don't drop turns silently.

## 9. Testing strategy

### Unit tests (`#[cfg(test)]` inside each module)

- `sentence_buffer` — every state-machine transition, multibyte char boundaries, code fence at stream end, CRLF normalization, nested terminators, empty input.
- `tokenizer` — exact counts against a fixed reference corpus committed at `tests/fixtures/tokenizer_corpus.json` (~20 short EN/ZH/code samples).
- `config` — TOML parse + merge precedence.
- `claude::api` — SSE event parser against captured fixture streams in `tests/fixtures/sse_streams/`.
- `claude::claude_code` — stream-json parser against captured NDJSON in `tests/fixtures/claude_code_ndjson/`.

### Integration tests (`tests/` directory)

- Full per-turn orchestration with `FakeTranslator` (deterministic mapping) + `FakeClaudeBackend` (scripted `ResponseChunk`s with realistic timing) + `MemoryBenchmarkSink`. Verifies emission ordering, cumulative token math, JSONL schema, and that conversation state advances on success and stays put on cancellation.
- Snapshot test: ingest a known fixture session's JSONL → `bench summary` output matches a committed `insta` snapshot.
- Failure-injection: fakes return errors at scripted points; assert the no-half-turns invariant.

### Live integration (`#[cfg(feature = "live")]`, off by default, never in CI)

- 3-turn fixture conversation against real Ollama + real Anthropic API; assert nonzero usage and successful JSONL write. For local sanity-checking only.

## 10. File layout

```
sigo/
├── Cargo.toml                          # workspace
├── README.md
├── crates/
│   ├── sigo-core/
│   │   ├── Cargo.toml
│   │   ├── assets/
│   │   │   └── claude2-tokenizer.json
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs
│   │       ├── conversation.rs
│   │       ├── error.rs
│   │       ├── orchestrator.rs
│   │       ├── tokenizer/
│   │       │   ├── mod.rs
│   │       │   └── claude.rs
│   │       ├── translator/
│   │       │   ├── mod.rs
│   │       │   ├── ollama.rs
│   │       │   └── prompts.rs
│   │       ├── claude/
│   │       │   ├── mod.rs
│   │       │   ├── api.rs
│   │       │   └── claude_code.rs
│   │       ├── stream/
│   │       │   ├── mod.rs
│   │       │   └── sentence_buffer.rs
│   │       └── benchmark/
│   │           ├── mod.rs
│   │           ├── jsonl_sink.rs
│   │           └── summary.rs
│   └── sigo-cli/
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── repl.rs
│           ├── display.rs
│           └── commands/
│               ├── bench.rs
│               ├── config_show.rs
│               └── doctor.rs
├── tests/
│   ├── fixtures/
│   │   ├── sse_streams/
│   │   ├── claude_code_ndjson/
│   │   └── tokenizer_corpus.json
│   ├── orchestrator_happy_path.rs
│   ├── orchestrator_failures.rs
│   └── bench_summary_snapshot.rs
└── docs/
    └── superpowers/
        └── specs/
            └── 2026-05-25-sigo-design.md
```

## 11. Success criteria

The framework is considered v1-complete when:

1. `sigo doctor` passes against a clean install with Ollama running and either an API key or Claude Code logged in.
2. A multi-turn REPL conversation with code blocks and mixed-length responses streams smoothly: code passes through verbatim, prose arrives in sentence-sized English chunks within a few hundred ms of the underlying Chinese token, ordering is correct.
3. Both `ApiBackend` and `ClaudeCodeBackend` produce comparable `TurnRecord` entries (modulo the `usage` fields the CLI backend doesn't expose).
4. `sigo bench summary` over a ≥20-turn session produces a calibration factor, cumulative totals, and a savings-percentage estimate. The number may be positive, negative, or near-zero — what matters is that the methodology is defensible and reproducible.
5. All offline tests pass in CI. Live tests run cleanly when manually invoked with `--features live`.
