# `sigo bench run` — Design Spec

**Date:** 2026-05-26
**Status:** Draft, awaiting user review

## 1. Purpose

Sigo already has the per-turn machinery to translate English→Chinese, send the Chinese prompt to Claude, capture authoritative token usage, and record everything to a rolling JSONL log. What it lacks is a way to drive that machinery across a corpus of prompts in one shot so we can answer the project's core hypothesis with data instead of one or two REPL turns:

> **Does Chinese prompting consume fewer Claude tokens than English prompting?**

`sigo bench run` is the scripted experiment harness that closes that gap. After a single invocation, the user has a markdown report with the headline number, a CSV they can drop into a notebook, and per-turn JSONL records.

The translation layer is not the deliverable; defensible numbers are. This spec describes the smallest delta to the existing v1 framework that produces those numbers.

## 2. Scope

### In scope

- New CLI subcommand `sigo bench run` that drives a configurable corpus through the orchestrator end-to-end.
- A bundled default corpus (~30 categorised prompts) shipped as an asset in `sigo-core`.
- A corpus loader tolerant of JSONL (`{category, prompt}`) and plain text (one prompt per line, category = `"general"`).
- Forced `control_mode = Full` during a run so every prompt produces a paired EN/ZH data point.
- Fresh orchestrator and fresh `ClaudeCodeBackend` per prompt — each prompt is a turn-0 of its own session so the Anthropic `input_tokens` field measures the prompt's own cost rather than accumulated conversation.
- Schema extension: `EnglishControlRun` gains `cache_read_tokens_reported` and `cache_write_tokens_reported` so the EN control side can be compared on total-input parity with the ZH side. `SCHEMA_VERSION` bumps to 2. Existing v1 records keep loading via `#[serde(default)]`.
- Per-run output: `report.md` (headline + per-category table + caveats) and `report.csv` (one row per prompt) under `$XDG_DATA_HOME/sigo/runs/<run-id>/`.
- Existing `turns.jsonl` continues to receive every record as before — `bench run` does not bypass it.
- Tests: corpus loader, report builder snapshot, v1→v2 migration, end-to-end integration with fakes.

### Out of scope (deliberate YAGNI)

- No multi-turn experimental shape — single-turn per fresh session is the only mode this command offers.
- No parallel execution across prompts — Ollama contention and JSONL ordering aren't worth the complexity.
- No alternate translators or backends beyond what `sigo` already supports.
- No automatic plotting — the CSV is the handoff to notebooks.
- No statistical hypothesis testing inside the report (p-values, confidence intervals). The report shows the raw means and Δ%; the user runs the stats they want.
- No corpus generation. The bundled corpus is hand-curated; no LLM-generated prompts.
- No retry-on-failure for individual prompts. Failures are logged to `errors.jsonl` in the run directory and the loop continues.

## 3. Subcommand surface

```
sigo bench run [--corpus <path>]
               [--backend <api|claude-code>]
               [--label <name>]
               [--limit <N>]
               [--out-dir <path>]
```

| Flag | Default | Effect |
|------|---------|--------|
| `--corpus` | bundled `default_corpus.jsonl` | Path to a JSONL or plain-text corpus. `-` reads stdin. |
| `--backend` | from resolved config | Overrides backend for this run only. |
| `--label` | absent | Slug appended to the run-id directory name so multiple runs are distinguishable. |
| `--limit` | no limit | Truncates the corpus to the first N entries — for smoke runs. |
| `--out-dir` | `$XDG_DATA_HOME/sigo/runs/<run-id>/` | Where `report.md`, `report.csv`, `errors.jsonl` are written. |

The run-id is `YYYY-MM-DDTHHMMSS[-<label>]` (UTC, no colons so the path is shell-safe on every platform).

Forced settings (not user-overridable for this subcommand):

- `control_mode = Full` — paired EN/ZH comparison is the whole point.
- Fresh `Orchestrator` and fresh backend instance per prompt — required to keep each prompt at turn 0 of its own session.

The translator settings (model, endpoint, timeout) come from the resolved config and are *not* overridable by flag in v1; users who want to swap translator pre-run should edit `sigo.toml` or pass `--config` (already supported by `sigo`).

## 4. Bundled default corpus

**Path:** `crates/sigo-core/assets/default_corpus.jsonl`. Loaded at runtime via `include_bytes!` so the asset is baked into the binary (matches how `claude2-tokenizer.json` already ships).

**Format (JSONL):**

```json
{"category": "coding-short", "prompt": "Write a Python function to reverse a string."}
{"category": "factual",      "prompt": "What's the difference between TCP and UDP?"}
```

**Composition target (≈30 entries):**

| Category | Count | Notes |
|----------|------:|-------|
| coding-short | 5 | One-function asks, ≤20 EN words. |
| coding-long | 5 | Multi-step implementation asks, 40–80 EN words. |
| refactor | 4 | "Refactor the following …" with a small code snippet inline. |
| debug | 4 | "Why does this fail …" with an error message inline. |
| explain | 4 | "Explain X in under 200 words." |
| factual | 4 | Short Q&A. |
| prose | 4 | Haiku/short essay/limerick — exercise the part of the input space where ZH is suspected to win most. |

Prompts are deliberately mid-length (10–80 EN words) so the per-prompt comparison sits in a useful range — short enough that translation latency is bearable, long enough that the prompt is the dominant input cost.

**Loader (`benchmark::corpus::load_corpus`):**

```rust
pub struct CorpusEntry { pub category: String, pub prompt: String }
pub fn load_corpus(path: Option<&Path>) -> Result<Vec<CorpusEntry>>
```

Heuristic: if `path` is `None`, return the bundled corpus parsed from the embedded asset. Otherwise read the file. If a non-empty line starts with `{`, parse the whole file as JSONL; otherwise treat each non-blank, non-`#`-comment line as a plain-text entry with `category = "general"`. Malformed JSONL lines are an error with `line:column` context.

## 5. Per-prompt execution loop

```rust
async fn run_corpus(
    cfg: &SigoConfig,
    corpus: Vec<CorpusEntry>,
    out_dir: &Path,
    rolling_sink: Arc<dyn BenchmarkSink>,
) -> Result<RunSummary>
```

For each `CorpusEntry`:

1. Build a fresh `Orchestrator` with `control_mode = Full`. New `session_id`, empty conversations.
2. Build a fresh backend instance. `ApiBackend` is stateless across calls, so a fresh instance is operationally identical to reusing one — we rebuild anyway for symmetry with the claude-code path and to keep the loop's behaviour uniform. `ClaudeCodeBackend::new(...)` resets its session mutex so the next call starts a new CLI session rather than `--resume`-ing the prior one.
3. `orchestrator.run_turn(&entry.prompt, &mut SilentSink)` — drop the streamed EN translation output on the floor; we don't need 30 responses scrolling past the user.
4. The returned `TurnRecord` is:
   - Cloned into an in-memory `Vec<TurnRecord>` for report generation.
   - Already appended to the rolling `turns.jsonl` by the orchestrator's sink — no separate write needed.
5. Print one progress line: `[12/30] coding-short · zh-in=14 en-in=18 (-22%) · 4.1s`. Progress uses the reported input-token gap so the user sees the hypothesis being measured live, even if individual prompts swing widely.

`SilentSink`:

```rust
struct SilentSink;
impl OutputSink for SilentSink {
    fn write(&mut self, _: &str) {}
}
```

**Error handling per prompt:**

- Translator failure (timeout, HTTP error, malformed response): record one JSONL line `{"prompt_index": u32, "category": String, "prompt": String, "stage": "translator" | "claude_open", "error": String, "timestamp": ISO8601}` into `errors.jsonl` under the run dir, continue with the next prompt. The orchestrator's `run_turn` returns `Err` before any TurnRecord is built in this case, so nothing lands in the rolling log either — the prompt is simply skipped.
- Claude failure mid-stream: the orchestrator already returns a TurnRecord with `incomplete: true`. The runner accepts it, marks it `incomplete` in the report, and continues. These records do land in `turns.jsonl` per existing behaviour.
- The runner itself never aborts the run on a single-prompt failure. Only fatal setup errors (config parse, sink file unwritable, corpus load failure) abort with a non-zero exit code.

## 6. Schema extension: `EnglishControlRun`

Existing v1 shape:

```rust
pub struct EnglishControlRun {
    pub english_response: String,
    pub prompt_tokens_reported: u32,
    pub response_tokens_reported: u32,
    pub duration_ms: u64,
}
```

v2 shape:

```rust
pub struct EnglishControlRun {
    pub english_response: String,
    pub prompt_tokens_reported: u32,
    pub response_tokens_reported: u32,
    #[serde(default)]
    pub cache_read_tokens_reported: Option<u32>,
    #[serde(default)]
    pub cache_write_tokens_reported: Option<u32>,
    pub duration_ms: u64,
}
```

`SCHEMA_VERSION` bumps from 1 to 2. The two existing v1 records in the live log keep loading because both new fields default to `None`.

The orchestrator's `run_english_control` already pattern-matches on `ResponseChunk::Done { usage, .. }`. The `Usage` struct already carries `cache_read` and `cache_write`. Populating the new fields is a 2-line change inside the existing function. The ZH side already records these on the `TurnRecord` itself, so this brings the EN control to parity.

This is the single non-trivial migration cost of `bench run`; everything else is additive. Worth doing because without it the headline "total input" comparison is one-sided and the answer to the hypothesis is unfair to either ZH or EN depending on which side's cache happens to be warmer.

## 7. Report

After the corpus loop finishes the runner writes two files into `out_dir/`.

### 7.1 `report.md`

Sections:

**Run header**
- run-id, started/finished (UTC), wall time
- backend, claude_model, translator_model
- corpus source (bundled or path), N attempted, N succeeded, N incomplete, N failed
- control_mode (always `full` for this command)

**Headline**
A two-column "ZH vs EN" block of mean values, each with `Δ%` and a one-word verdict (`ZH wins` / `EN wins` / `wash` — `wash` is `|Δ| < 5%`):

- Reported `input_tokens` (uncached new input)
- Total input = `input_tokens + cache_read + cache_write`
- Reported `output_tokens`
- Local-tokenizer prompt count (same metric in both languages, calibration-free)
- Wall-clock per turn

The headline answers the hypothesis at a glance. The "uncached" and "total" rows together protect against the claude-code cache-scaffolding effect described in section 9.

**Per-category breakdown**
Markdown table, one row per category that appears in the corpus:

```
| Category      | N | EN-in | ZH-in | Δ%   | EN-tot | ZH-tot | Δ%   | EN-out | ZH-out | Δ%   |
|---------------|---|-------|-------|------|--------|--------|------|--------|--------|------|
| coding-short  | 5 |  18.4 |  14.2 | -23% |  91312 |  91298 |  -0% |   840  |  1100  | +31% |
```

Reveals whether savings are uniform or domain-dependent. Output-token deltas are reported but framed as informational — the hypothesis is about input cost.

**Caveats**

Auto-generated, listing whichever apply to this run:

- "Reported `input_tokens` for the `claude-code` backend excludes cached system-prompt scaffolding (~90k tokens per turn). The 'Total input' row is the fair comparison." — emitted only when `backend = claude-code`.
- "N prompts failed in translation or Claude. See `errors.jsonl`." — emitted only when failures > 0.
- "Sample size is N prompts; treat means as point estimates, not statistically significant differences. The CSV is the input to your stats package of choice." — always emitted.
- "ZH responses were translated back to EN by the local translator; the EN you'd read is not a like-for-like answer match to the EN control's response. The token cost comparison is unaffected by this, but a quality comparison is not in scope of `bench run`." — always emitted.

### 7.2 `report.csv`

One row per attempted prompt. Columns:

```
run_id, prompt_index, category, prompt,
zh_input_reported, zh_output_reported, zh_cache_read, zh_cache_write, zh_total_input,
en_input_reported, en_output_reported, en_cache_read, en_cache_write, en_total_input,
zh_prompt_local, en_prompt_local,
zh_response_local, zh_response_reported,
delta_input_pct, delta_total_input_pct, delta_output_pct,
translation_in_ms, translation_out_ms_total, claude_total_ms, turn_total_ms,
incomplete, errors
```

`delta_*_pct` columns are computed in the runner so notebooks don't have to. `errors` is a `;`-joined list of `turn_errors` for the row.

### 7.3 Stdout summary

On completion, one final line to stdout:

```
bench run complete: 28/30 prompts succeeded · ZH input -19% vs EN · report: /home/.../runs/2026-05-26T143012-corpus30/report.md
```

## 8. Testing

### Unit

- `benchmark::corpus::load_corpus` — JSONL parsing happy path, malformed line yields `line:col` error, plain-text fallback, `#`-comment skipping, blank-line skipping, bundled corpus parses cleanly (asserted via `include_str!` test).
- `benchmark::run_report::build_report` — given a hand-built `Vec<TurnRecord>` with known token numbers across two categories, asserts the headline math and per-category math. Markdown and CSV outputs snapshot-tested with `insta`.
- `benchmark::turn_record` v1→v2 migration — frozen v1 JSONL fixture (a copy of one current real record) deserialises into v2 `TurnRecord` with cache fields = `None`.

### Integration

- `tests/bench_run_integration.rs`: build a 3-entry in-memory corpus, wire `FakeTranslator` + `FakeBackend` with scripted EN-translation pairs and scripted usage numbers, run the runner against a `tempfile::TempDir` out-dir, then assert:
  - 3 records in the rolling JSONL.
  - `report.md` exists, contains the expected headline numbers, lists 2 categories.
  - `report.csv` has 3 data rows plus header.
  - `errors.jsonl` does not exist (no failures).
- A separate integration test injects one mid-stream Claude error on prompt #2 and asserts:
  - The runner does not abort.
  - `report.md` records 2 succeeded / 1 incomplete.
  - `errors.jsonl` does not exist (the incomplete one is in the rolling log, not `errors.jsonl`).
- A third integration test injects one *translator* error on prompt #2 and asserts:
  - The runner does not abort.
  - `errors.jsonl` contains one entry with prompt and error.
  - `report.md` reports 2 succeeded / 1 failed.

### Live (off by default)

No new live tests in v1. The existing `live` feature covers translator and API round-trips; `bench run` against a real Claude backend is exercised by the user when they actually run the experiment, not in CI.

## 9. Methodology notes (carried into the report's caveats)

These are the known limits of the experiment this command runs. They are mentioned here so the spec is honest about what `bench run` does and does not prove.

- **Reported `input_tokens` excludes cached input.** For `api` backend without an explicit cache control, this is the full new input cost — clean. For `claude-code` backend, the Claude Code CLI sends ~90k tokens of system-prompt scaffolding per session that is heavily cached. The reported `input_tokens` therefore measures the prompt's marginal cost, not its total cost; the "Total input" row adds back `cache_read + cache_write` to give a fair comparison. Both rows are reported.
- **Single-turn per session means no caching benefit accumulates across turns.** This is intentional — we want to measure the prompt itself, not "how much does ZH benefit from cache reuse over a long conversation". A future `bench run --multi-turn` flag could measure that separately; out of scope for v1 of this command.
- **The translator is a confound.** Ollama Qwen2.5 may produce a slightly different ZH translation than another Qwen quant or Gemma would. The corpus, model, and translator are all fixed and reported in the run header so a re-run is comparable to itself; cross-translator comparisons require a fresh run.
- **Output tokens are reported but not the headline.** A ZH response that's denser per token but the same length in words still costs more output tokens than an EN response; this is interesting but tangential to the prompt-token hypothesis.

## 10. File layout

```
crates/
├── sigo-core/
│   ├── assets/
│   │   ├── claude2-tokenizer.json        # existing
│   │   └── default_corpus.jsonl          # NEW
│   └── src/
│       └── benchmark/
│           ├── corpus.rs                 # NEW (loader)
│           ├── run_report.rs             # NEW (md + csv builder)
│           ├── jsonl_sink.rs             # unchanged
│           ├── summary.rs                # unchanged
│           ├── turn_record.rs            # edit: extend EnglishControlRun, bump SCHEMA_VERSION
│           └── mod.rs                    # edit: re-export corpus + run_report
│       └── orchestrator.rs               # edit: populate cache_read/write in run_english_control
└── sigo-cli/
    └── src/
        ├── cli.rs                        # edit: add Run variant to bench subcommand
        └── commands/
            ├── bench_run.rs              # NEW
            └── mod.rs                    # edit: register module
```

## 11. Success criteria

The subcommand is complete when:

1. `sigo bench run --limit 3` against the bundled corpus and the user's existing `sigo.toml` (claude-code backend) succeeds end-to-end and produces a readable `report.md`.
2. The report's "Total input" headline differs from the "Reported input" headline by approximately the magnitude of the cache fields (i.e., the caveat about claude-code caching is visible in the numbers, not just text).
3. `sigo bench run` (no flags) against the full bundled corpus runs to completion, succeeds on >=27/30 prompts, and the headline Δ% on at least one of the three input metrics is a non-trivial number with a clear direction.
4. The CSV loads cleanly into pandas (`pd.read_csv`) without column-type surprises.
5. All offline unit and integration tests pass.
6. The two existing v1 records in the live `turns.jsonl` continue to load via `bench summary` and `bench show` after the schema bump.
