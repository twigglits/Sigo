# Sigo

Sino-Anglo translation layer that routes English prompts through a local
Ollama-hosted translator (Qwen / Gemma 3) into Chinese, sends them to
Claude (Anthropic API or local `claude` CLI), and translates the
streamed Chinese response back to English. Every turn is recorded so
you can benchmark Claude's token cost on Chinese vs English prompts.

## Architecture

Two-crate Cargo workspace.

- `crates/sigo-core` — library: traits (`Translator`, `ClaudeBackend`,
  `Tokenizer`, `BenchmarkSink`), the per-turn orchestrator, the
  sentence-buffer streaming transformer, and concrete adapters.
- `crates/sigo-cli` — binary: `clap`-based CLI, `rustyline` REPL,
  config loader, and `bench` / `doctor` subcommands.

See `docs/superpowers/specs/2026-05-25-sigo-design.md` for the full design
and `docs/superpowers/plans/2026-05-25-sigo-v1.md` for the implementation
plan.

## Requirements

- Rust 1.75+
- A running Ollama with a chat model pulled (e.g. `qwen2.5:7b`,
  `qwen3:14b`, `gemma3:12b`).
- One of:
  - `ANTHROPIC_API_KEY` set in your environment (for `api` backend)
  - The `claude` CLI on PATH and logged in (for `claude-code` backend)

## Install

```bash
cargo build --release
# binary at target/release/sigo
```

## First-run setup

1. Install + start Ollama: see https://ollama.com
2. Pull a translation model:
   ```bash
   ollama pull qwen2.5:7b
   ```
3. Set your Anthropic key (skip if you're using `claude-code`):
   ```bash
   export ANTHROPIC_API_KEY=sk-...
   ```
4. Run `sigo doctor` to verify everything's reachable:
   ```bash
   ./target/release/sigo doctor
   ```

## Configuration

Sigo reads `./sigo.toml` (cwd) overriding `$XDG_CONFIG_HOME/sigo/config.toml`.

```toml
[translator]
provider = "ollama"
endpoint = "http://localhost:11434"
model = "qwen2.5:7b"
timeout_seconds = 60

[claude]
backend = "api"                    # or "claude-code"
model = "claude-sonnet-4-6"
max_tokens = 4096

[claude.claude_code]
binary = "claude"
extra_args = []

[benchmark]
control_mode = "prompt-only"       # off | prompt-only | full

[repl]
verbose = false
```

## Usage

Start the REPL:
```bash
sigo
```

Type English, get English. The turn footer shows token counts and the
estimated savings vs the English baseline.

### Subcommands

```bash
sigo doctor                       # check setup
sigo config-show                  # resolved effective config
sigo bench summary                # aggregate stats from the JSONL log
sigo bench show <session> <turn>  # full record
sigo bench export --format csv    # for notebook analysis
sigo bench run                          # run a corpus end-to-end, write report
sigo bench run --limit 5                # smoke run over the first 5 prompts
sigo bench run --corpus my.jsonl        # use a custom prompt file
sigo --backend api bench run --limit 3  # override backend via top-level flag
```

### REPL slash-commands

- `/help` — list commands
- `/quit`, `/exit` (or Ctrl-D) — leave
- `/verbose` — toggle the ZH bridge + token panel display
- `/reset` — clear conversation, new session id
- `/control-mode <off|prompt-only|full>` — change for subsequent turns
- `/model translator <name>` / `/model claude <name>` — hot-swap models
- `/backend <api|claude-code>` — hot-swap backend
- `/bench` — quick summary of the current session

## Benchmark methodology

- **Live Chinese run.** Each REPL turn translates EN→ZH and runs the
  Chinese conversation against Claude. Claude's response stream tells
  us the authoritative input/output token counts.
- **English control.** Each turn we keep a parallel English transcript.
  - `control_mode = "prompt-only"`: local-tokenize the English
    transcript (Claude 2 tokenizer via `claude-tokenizer` crate) — no
    extra Claude calls.
  - `control_mode = "full"`: fire a parallel English Claude run per
    turn and capture its authoritative usage. Doubles API cost.
- **Calibration.** Local-tokenizer counts approximate Claude's actual
  tokenization (Claude 2 BPE in the same family as later Claudes). The
  ratio `chinese_local / chinese_reported` per turn is a calibration
  factor we use to convert English-local counts into estimated
  authoritative tokens for the savings percentage.

The JSONL log is rolling and append-only at
`$XDG_DATA_HOME/sigo/turns.jsonl`. Each line is one `TurnRecord`.

### Scripted bench runs

`sigo bench run` drives a corpus of prompts through the orchestrator with
`control_mode=full` and writes a per-run report:

- `$XDG_DATA_HOME/sigo/runs/<run-id>/report.md` — headline ZH vs EN
  comparison and per-category breakdown.
- `report.csv` next to it — one row per prompt for notebook analysis.
- `errors.jsonl` — only created if some prompts failed.

The bundled default corpus is 30 prompts across seven categories
(coding-short, coding-long, refactor, debug, explain, factual, prose).
Pass `--corpus <path>` for a custom JSONL (`{"category", "prompt"}`) or
plain text (one prompt per line). `--limit N` runs only the first N for a
smoke test.

Each prompt is run as turn 0 of a fresh session so the reported `input_tokens`
isolates the prompt's own cost. The `claude-code` backend's cached
system-prompt scaffolding shows up under `cache_read_tokens_reported` and is
reflected in the report's "Total input" row.

## Development

```bash
cargo build --workspace
cargo test --workspace
```

There are 36 unit + integration tests covering the conversation types,
the bundled tokenizer, the sentence-buffer state machine, the Anthropic
SSE event parser, the Claude Code NDJSON parser, the orchestrator
pipeline (happy path + full control mode + stream-without-Done), the
JSONL sink roundtrip, and the bench summary calibration math.

Live tests against real Ollama + real Anthropic API are gated behind
`--features live` and are not run by default:

```bash
cargo test -p sigo-core --features live
```

## License

MIT
