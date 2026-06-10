# Sigo

[![CI](https://github.com/twigglits/Sigo/actions/workflows/ci.yml/badge.svg)](https://github.com/twigglits/Sigo/actions/workflows/ci.yml)

Sino-Anglo translation layer for Claude. Sigo routes your English prompt through a
local Ollama translator (Qwen / Gemma 3) into Chinese, sends the Chinese to Claude
(Anthropic API or the local `claude` CLI), and streams the Chinese answer back
through the translator into English. Every turn is recorded so you can benchmark
Claude's token cost on Chinese vs English prompts.

If the translator is unreachable or the model isn't pulled, Sigo **stops with an
actionable error** rather than silently sending English — the translation layer is
the point, so it never degrades quietly.

## Quickstart

### 1. Docker Compose (recommended — zero local toolchain)

Bundles Ollama, auto-pulls the translator model on first run, and starts Sigo.

```bash
git clone https://github.com/twigglits/Sigo && cd Sigo
cp .env.example .env          # add your ANTHROPIC_API_KEY
docker compose run --rm sigo                 # interactive REPL
echo "explain this regex: ^\d{3}-\d{4}$" | docker compose run --rm -T sigo chat
```

First run pulls the Ollama image and the ~4.7 GB `qwen2.5:7b` model (one-time,
persisted in a volume). NVIDIA GPU acceleration is opt-in:

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml run --rm sigo
```

### 2. Install script (prebuilt binary)

```bash
curl -fsSL https://raw.githubusercontent.com/twigglits/Sigo/main/install.sh | sh
```

Installs the `sigo` binary for your platform (Linux x86_64/aarch64, macOS
x86_64/aarch64) to `~/.local/bin`, verifying its checksum. Then install
[Ollama](https://ollama.com), `ollama pull qwen2.5:7b`, set `ANTHROPIC_API_KEY`,
and run `sigo doctor`.

### 3. From source

```bash
cargo build --release        # binary at target/release/sigo
```

Requires Rust 1.88+. Same external setup as the install-script path.

## Requirements

- A running Ollama with a chat model pulled (e.g. `qwen2.5:7b`, `qwen3:14b`,
  `gemma3:12b`). The Docker path provides this for you.
- One of:
  - `ANTHROPIC_API_KEY` set in your environment (the `api` backend — default), or
  - the `claude` CLI on PATH and logged in (the `claude-code` backend; native runs).

## First-run check

```bash
sigo doctor      # verifies Ollama, the model, your Claude auth, the tokenizer, and python3
```

## Architecture

Two-crate Cargo workspace:

- `crates/sigo-core` — library: traits (`Translator`, `ClaudeBackend`, `Tokenizer`,
  `BenchmarkSink`), the per-turn orchestrator, the sentence-buffer streaming
  transformer, the ZH whitespace compactor (`compact.rs`), the code-masking
  layer that keeps code out of the local model's hands (`translator/mask.rs`),
  and concrete adapters.
- `crates/sigo-cli` — binary: the `clap` CLI, the `rustyline` REPL, the one-shot
  `chat` command, config loading (files + `SIGO_*` env), and the `bench` / `doctor`
  subcommands.

## Configuration

Sigo reads `./sigo.toml` (cwd) overriding `$XDG_CONFIG_HOME/sigo/config.toml`.

Any setting can also be overridden by an environment variable (highest precedence
after CLI flags) — convenient for containers:

| Env var                    | Setting                  |
|----------------------------|--------------------------|
| `SIGO_TRANSLATOR_ENDPOINT` | `translator.endpoint`    |
| `SIGO_TRANSLATOR_MODEL`    | `translator.model`       |
| `SIGO_TRANSLATOR_STYLE`    | `translator.style`       |
| `SIGO_CLAUDE_BACKEND`      | `claude.backend`         |
| `SIGO_CLAUDE_MODEL`        | `claude.model`           |
| `SIGO_CLAUDE_MAX_TOKENS`   | `claude.max_tokens`      |
| `SIGO_CONTROL_MODE`        | `benchmark.control_mode` |
| `SIGO_LOG_PATH`            | `benchmark.log_path`     |

Precedence (low → high): built-in defaults < `$XDG_CONFIG_HOME/sigo/config.toml`
< `./sigo.toml` < `SIGO_*` env vars < CLI flags. A starter config is in
`sigo.toml.example`.

```toml
[translator]
provider = "ollama"
endpoint = "http://localhost:11434"
model = "qwen2.5:7b"
timeout_seconds = 60
style = "terse"                    # terse (token-minimizing, default) | fluent (baseline)

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

[pricing]
# Dollars per million tokens — used by `--eval coding` to compute marginal cost.
# Defaults match Sonnet list price; override for other models or negotiated rates.
input_per_mtok       = 3.0
output_per_mtok      = 15.0
cache_read_per_mtok  = 0.30
cache_write_per_mtok = 3.75
```

## Usage

Start the REPL:
```bash
sigo
```

Type English, get English. The turn footer shows the per-turn latency, the
input tokens Claude reported for the Chinese prompt, and a local `o200k_base`
proxy count for the English baseline — e.g.
`[turn 0 · 1873 ms · ZH-in 41 reported vs EN-proxy 27 local]`. Sigo shows the
two counts side by side rather than inventing a single "savings" number.

### Subcommands

```bash
sigo doctor                       # check setup
sigo config-show                  # resolved effective config
sigo chat "your prompt"           # one-shot: one turn, English answer to stdout
echo "your prompt" | sigo chat    # same, reading the prompt from stdin
sigo bench summary                # aggregate stats from the JSONL log
sigo bench show <session> <turn>  # full record
sigo bench export --format csv    # for notebook analysis
sigo bench run                          # run a corpus end-to-end, write report
sigo bench run --limit 5                # smoke run over the first 5 prompts
sigo bench run --corpus my.jsonl        # use a custom prompt file
sigo --backend api bench run --limit 3  # override backend via top-level flag
sigo bench run --eval coding            # objective coding benchmark (bundled HumanEval):
                                        # runs each task through BOTH arms — direct English
                                        # vs. the full EN→ZH→Claude→EN pipeline — and scores
                                        # the model's code by executing it against the task's tests
sigo bench run --eval coding --limit 5  # smoke run over the first 5 tasks
sigo bench run --eval coding --corpus my_humaneval.jsonl  # custom HumanEval-format corpus
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

## Token minimization — and what is honestly known

Three prompt-side mechanisms minimize what Claude is billed for; all report
into the bench artifacts so their effects stay attributable and falsifiable:

- **Terse translation register (default).** Plain fluent translation does NOT
  save tokens — an English-optimized BPE penalizes CJK, so a faithful fluent
  rendering costs *more* than the English original. The default
  `style = "terse"` instead asks the local translator for maximally concise
  written Chinese (简练书面语) that preserves every fact, constraint, number,
  name, and negation. Set `style = "fluent"` to run the baseline register
  (kept so paired runs can attribute savings to the register rather than to
  translation per se).
- **Whitespace compactor with a never-worse guard.** The outbound ZH prompt is
  deterministically compacted (trailing whitespace, newline runs, interior space
  runs, CJK↔Latin boundary spaces — never inside code, never on lines without
  CJK, never URLs/paths). Each turn both forms are counted with the proxy and
  the cheaper one is sent, so the step cannot lose tokens. The pre-compaction
  count is recorded per turn (`chinese_prompt_tokens_precompact_local`).
- **Translate-not-answer protocol + structural code masking.** A live corpus
  sweep caught the local translator *answering* instruction-shaped prompts
  instead of translating them ("Explain X" reached Claude as the 7B's
  explanation of X) and, separately, solving or dropping code it was supposed
  to pass through. The translator now sends `<source>`-wrapped text with
  few-shot demonstrations, and code spans never reach the model at all — they
  are masked behind sentinels in Rust and reinstated byte-for-byte. A dropped
  sentinel is repaired by appending the code; a duplicated one fails the turn
  loudly. Known residual: trivial arithmetic bait ("What is 2+2?") can still
  get answered.

**Measured results** (`o200k_base` proxy, live `qwen2.5:7b`, the bundled
30-chat + 10-HumanEval corpora through the exact production path —
reproduce with `cargo run -p sigo-core --example measure_pipeline`):

| Pipeline | vs direct English |
|---|---|
| Original (fluent register, unprotected protocol) | **+36%**, with silent prompt corruption |
| Shipped (terse + protocol + masking + compactor) | **+4.8%** overall · +9.7% chat · **+0.3% code (parity)** · zero detected constraint losses |
| Hand-picked verbose prose prompts (A/B) | **−22% to −51%** |

The pattern: terse ZH wins on verbose, redundancy-rich prose, ties on
code-dominant prompts, and still loses slightly on already-terse technical
English.

What may NOT be claimed from this: all numbers above are `o200k_base` **proxy**
counts, not Claude's non-public tokenizer; the only live paired bench to date
(N=2, fluent register) found **EN cheaper on every layer**, and the terse
pipeline has not yet been live-benched; output tokens dominate cost 3–5× and
are not controlled by prompt-side changes; and terse-vs-verbatim conflates
compression with language — attributing the split needs a terse-English control
arm, which does not exist yet. The verdict instrument remains
`sigo bench run --eval coding` (cost per passing task, paired, CIs).

## Benchmark methodology

- **Live Chinese run.** Each REPL turn translates EN→ZH and runs the
  Chinese conversation against Claude. Claude's response stream tells
  us the authoritative input/output token counts.
- **Local proxy counts.** Every turn also counts the English and Chinese
  prompts with a local `o200k_base` BPE tokenizer (`tiktoken-rs`). Claude's
  tokenizer is non-public, so these are a **directional proxy**, not
  authoritative numbers — handy for a quick EN-vs-ZH ratio without spending
  tokens.
- **English control.** Each turn keeps a parallel English transcript.
  - `control_mode = "prompt-only"`: the English baseline is the local proxy
    count only — no extra Claude calls.
  - `control_mode = "full"`: additionally fire a parallel English Claude run
    per turn and capture its authoritative usage. Doubles API cost, but every
    layer is then a real-vs-real comparison.
- **No invented "savings."** Sigo shows the reported Chinese cost and the
  English proxy/control side by side and lets you compare them; it deliberately
  does **not** synthesize a single "estimated savings %" by calibrating one
  tokenizer against another. For an authoritative, objectively-scored paired
  comparison, use `--eval coding` (below).

The JSONL log is rolling and append-only at
`$XDG_DATA_HOME/sigo/turns.jsonl`. Each line is one `TurnRecord`.

### Scripted bench runs

`sigo bench run` drives a corpus of prompts through the orchestrator with
`control_mode=full` and writes a per-run report:

- `$XDG_DATA_HOME/sigo/runs/<run-id>/report.md` — headline ZH vs EN
  comparison and per-category breakdown. The header records the translator
  style and the compactor's aggregate token savings, so runs with different
  registers are never conflated.
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

### Coding eval (`--eval coding`)

`sigo bench run --eval coding` runs each task in the bundled HumanEval corpus (or
a custom corpus with `--corpus`) through **both arms in parallel**: a direct English
prompt and the full EN→ZH→Claude→EN pipeline. Each model response is scored by
executing the generated Python against the task's test suite.

**Outputs** written to `$XDG_DATA_HOME/sigo/runs/<run-id>/`:

- `eval_report.md` — headline comparison table and correctness summary.
- `eval_report.csv` — one row per task per arm for notebook analysis.

**Metrics — three paired layers**, each with a bootstrap percentile 95% CI and a
ZH win-rate:

1. **Input tokens (proxy)** — local o200k\_base BPE counts. These are a **proxy**
   for Claude's tokenizer, which is non-public. Treat them as directional estimates,
   not authoritative numbers.
2. **Input tokens (reported, uncached)** — the authoritative counts reported by
   Claude's API for each live run. These are the numbers to trust.
3. **Marginal dollar cost** (input + output tokens at configured rates). Does not
   include cache read/write charges, which are asymmetric across the paired arms.

**Correctness**: pass-rate per arm with Wilson 95% confidence intervals.
**Cost per passing task**: mean marginal cost ÷ pass-rate (∞ if no tasks pass).

**Round-trip fidelity**: a local Ollama judge back-translates the ZH prompt and
scores **constraint recall** 0–10 — whether every fact, constraint, number,
name, negation, and instruction survived — explicitly ignoring brevity, tone,
and phrasing (so the terse register isn't punished for dropping politeness).
Diagnostic only, never a gate: the same local model usually does both
translation directions, so correlated errors can cancel and inflate scores.

**Known limitations and safety notes:**

- `--samples` currently supports only `1` (pass@1). Higher values (pass@k) are
  reserved but not yet implemented.
- The eval executes model-generated Python locally. It is sandboxed in depth: each
  solution runs under [bubblewrap](https://github.com/containers/bubblewrap) when
  available (fresh namespaces with **no network**, a read-only system, and a private
  `/tmp`), and always behind an in-process guard that neutralises the common
  shell/file/exec entry points and caps address space. bubblewrap is used only if a
  start-up probe confirms it works in your environment; otherwise the in-process guard
  still applies. The guard is best-effort, not a security boundary — for a genuinely
  untrusted corpus, install `bwrap` (or run inside a throwaway VM/container).
  `python3` must be on PATH (verified by `sigo doctor`).
- N is typically small; bootstrap CIs are indicative, not tight.

## Development

```bash
cargo build --workspace
cargo test --workspace
```

The workspace carries a broad unit + integration suite covering the
conversation types, the `o200k_base` tokenizer proxy, the sentence-buffer
state machine, the whitespace compactor (golden adversarial corpus: unfenced
indented Python, markdown markers, quoted CJK literals, URL/path boundaries,
idempotency, tokens-never-increase), the code-masking roundtrip, the
translate-not-answer request shape, the Anthropic SSE event parser, the Claude
Code NDJSON parser, the orchestrator pipeline (happy path + full control mode
+ compaction guard + raw-assistant-history invariant), the JSONL sink
roundtrip, and the bench summary aggregation (which reports raw counts without
inventing an estimate).

Live tests against real Ollama + real Anthropic API are gated behind
`--features live` and are not run by default:

```bash
cargo test -p sigo-core --features live
```

### Releasing

Releases are cut from the **Version** workflow (Actions → Version → Run
workflow). It derives the SemVer increment from the conventional commits since
the last tag — breaking change → major (minor while on 0.x), `feat` → minor,
anything else → patch — or takes an explicit `patch`/`minor`/`major` override
(`major` is the only way to cut 1.0.0 from 0.x). The same computation runs
locally via `scripts/next-version.sh [auto|patch|minor|major]`.

The workflow bumps every workspace version in lockstep (`Cargo.toml`, the
`sigo-cli` → `sigo-core` dependency pin, `Cargo.lock`), commits
`chore(release): vX.Y.Z`, tags, and chains into the **Release** workflow, which
refuses to ship unless the tag matches the crate version, then builds the
binaries, the Docker image, and the GitHub Release with generated notes.
Manually pushed `v*` tags still trigger Release directly and are held to the
same tag-matches-version guard.

## License

MIT
