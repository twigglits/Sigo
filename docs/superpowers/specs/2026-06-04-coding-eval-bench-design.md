# `sigo bench run --eval coding` — Design Spec

**Date:** 2026-06-04
**Status:** Draft, awaiting user review

## 1. Purpose

`sigo bench run` (full control mode) already answers a narrow question honestly:
for a corpus of prompts it captures Claude's authoritative input/output token
usage for the Chinese path and for a paired English control run. What it does
**not** do is tell us whether routing through Chinese is *worth it* — because it
measures only prompt-token deltas, de-emphasises output tokens, never combines
them into a cost, and never checks whether the answer is still correct.

This spec adds an **objective coding-evaluation mode** that closes that gap and
makes the headline numbers ones we'd defend in public. The core hypothesis is
unchanged but stated honestly:

> **Does routing an English coding request through Chinese cost fewer Claude
> tokens / dollars per *correct* answer than just asking in English?**

The 2026 literature predicts the answer is "no, or worse" for Claude
specifically (Claude's tokenizer is English-optimised; CJK is penalised). Rather
than assume that, this mode builds the harness that measures it cleanly, with
confidence intervals and an objective pass/fail signal, so the result — whichever
way it falls — is trustworthy.

### Grounding (why these specific metrics)

- **Tokenizer, not language, determines efficiency.** Chinese is cheaper only on
  Chinese-trained tokenizers (GLM ZH/EN ≈ 0.92); on English-optimised BPE
  (`cl100k_base` +15%, `o200k_base` similar) Chinese costs *more* per equivalent
  prompt. (Petrov et al. 2023; "Mythbuster", arXiv 2604.14210.)
- **Total cost must include output, price-weighted.** Output tokens price ~3–5×
  input and frequently *expand* in the non-English arm; an input-only headline
  can read "win" while the bill rises. (arXiv 2606.03618 §"From Tokens to
  Dollars": dollar outcomes −12.4% / −0.4% / **+15.1%** across backends.)
- **Normalise by success.** A language that produces fewer tokens but fails more
  costs more per *usable* answer. Expected cost per successful task =
  cost ÷ resolution-rate. ("Mythbuster"; Chinese lowered SWE-bench resolution
  4.5–9.9 pp.)
- **Authoritative tokenisation needs the real tokenizer.** Claude's tokenizer is
  non-public; only the API gives true counts. We have no API key, so we use a
  *labelled proxy* (`o200k_base`) for offline estimates and Claude-reported usage
  for the authoritative figures. (Sander Land; Javier Rando.)

## 2. Scope

### In scope

- New flag set on the existing subcommand:
  `sigo bench run --eval coding [--samples N]`.
- A verifiable-coding corpus type (`CodingTask`) and loader; a bundled
  ~100-task HumanEval subset shipped as a `sigo-core` asset; `--corpus` accepts a
  full HumanEval-format JSONL.
- Replace the Claude-2 BPE local tokenizer (`claude-tokenizer`) with
  `tiktoken-rs` `o200k_base`, reported **as a proxy**. Remove the now-invalid
  cross-language calibration in `summary.rs`.
- Objective execution: extract the model's code from each arm's answer, run it
  against the task's official `test` via `python3` under a timeout, classify the
  outcome.
- Three layered, paired metrics with confidence intervals: input-token Δ,
  price-weighted dollar Δ, and cost-per-passing-task; plus a round-trip prompt
  fidelity diagnostic scored by a local Ollama judge.
- A new `[pricing]` config block.
- Eval report (`eval_report.md` + `eval_report.csv`) under the run directory.
- Tests: corpus loader, code extraction, execution classification, cost math,
  seeded-bootstrap determinism, integration with fakes, gated live run.

### Out of scope (deliberate YAGNI)

- **Non-Python corpora.** v1 executes Python tasks only (HumanEval/MBPP are
  Python). Multi-language execution is a later flag.
- **API backend / `count_tokens`.** No API key available; authoritative counts
  come from the `claude-code` backend's reported usage.
- **Answer-equivalence judging.** v1 fidelity is *prompt* round-trip only
  (EN→ZH→EN); objective task success already covers answer quality.
- **Plotting.** The CSV is the notebook handoff, matching existing `bench run`.
- **Long-context and prose arms.** Dropped in favour of objective rigour; can be
  added as separate corpora later.
- **pass@k > configurable.** `--samples` exists but defaults to 1 (pass@1).

## 3. Subcommand surface

```
sigo bench run --eval coding
               [--samples <N>]          # generations per task per arm; default 1 (pass@1)
               [--corpus <path>]        # HumanEval-format JSONL; default = bundled subset
               [--label <name>]
               [--limit <N>]
               [--out-dir <path>]
```

`--eval coding` selects the coding schema + the eval report. Without it, `bench
run` behaves exactly as today. `control_mode = Full` is forced (already the case)
because the EN/ZH pairing is the whole point.

## 4. The two-arm experiment

Each task is evaluated through two arms, scored by the **same** official test:

- **EN arm (baseline):** the English `prompt` sent straight to Claude. This is
  the orchestrator's existing English control run — no translation. It answers
  "what if I just ask in English?"
- **ZH arm (full Sigo):** English `prompt` → Ollama EN→ZH → Claude → ZH→EN. This
  is the complete pipeline a Chinese-speaking developer's request would take.

The orchestrator's Full mode already returns both answers and both authoritative
usages in one `TurnRecord` (`chinese_response` + `english_control_run`). Coding
eval is therefore a **post-turn scoring hook**, not a rewrite of the orchestrator.

**Translation of the ZH-arm prompt is deliberately end-to-end.** The whole
`prompt` (signature + docstring + doctests) goes through the existing EN→ZH
translator, whose system prompt already instructs it to preserve fenced/inline
code, identifiers, and `>>>` doctest lines while translating prose. If the local
model corrupts the signature or spec, that is a **real cost of the approach** and
the objective test will catch it (as `NoCodeExtracted`, `CompileError`, or
`AssertFail`). We do not hand-protect the signature; doing so would measure an
idealised pipeline, not the real one.

## 5. Corpus: `CodingTask`

**Path (bundled):** `crates/sigo-core/assets/humaneval_sample.jsonl`, embedded
via `include_bytes!` like the existing default corpus.

**Format (JSONL), HumanEval-compatible:**

```json
{"task_id":"HumanEval/0","category":"coding-verifiable","prompt":"from typing import List\n\n\ndef has_close_elements(numbers: List[float], threshold: float) -> bool:\n    \"\"\" Check if ... \"\"\"\n","entry_point":"has_close_elements","test":"def check(candidate):\n    assert candidate([1.0, 2.0], 0.5) == False\n"}
```

```rust
pub struct CodingTask {
    pub task_id: String,
    pub category: String,     // default "coding-verifiable"
    pub prompt: String,       // signature + English docstring (+ doctests)
    pub entry_point: String,  // function name the test calls
    pub test: String,         // Python defining `check(candidate)` with asserts
}
```

**Loader (`benchmark::coding_corpus::load_coding_corpus`):** parses JSONL with
the four required fields; a malformed/short line is an error with `line` context;
empty corpus is an error. Selected only under `--eval coding`; the plain
`{category, prompt}` path is untouched for non-eval runs. Bundled subset target:
**100 tasks** (HumanEval is 164; we ship a representative 100 to keep a full run
tractable on the `claude-code` backend, and document that `--corpus` takes the
full set).

## 6. Tokenizer replacement

Drop `claude-tokenizer = "0.3.0"` (Claude-2 BPE; byte-fallback inflates Chinese
~2–3×). Add `tiktoken-rs` and implement `TokenizerProxy` over `o200k_base`:

```rust
// crates/sigo-core/src/tokenizer/proxy.rs
pub struct TokenizerProxy { bpe: CoreBPE }   // o200k_base, embedded offline
impl Tokenizer for TokenizerProxy {
    fn count_tokens(&self, text: &str) -> Result<u32> { Ok(self.bpe.encode_with_special_tokens(text).len() as u32) }
}
```

The `Tokenizer` trait is unchanged; only the concrete impl changes. Every report
labels these counts **"proxy (o200k_base)"**, never "Claude". `o200k_base` is
GPT-4o's tokenizer — an English-optimised BPE in the same family as Claude's, and
the proxy the cited papers use for EN/ZH comparison.

**Remove the calibration fallacy.** `summary.rs` currently derives a calibration
factor from Chinese (`reported/local`) and applies it to English local counts —
invalid across languages. With a sane proxy plus authoritative reported counts,
delete `calibration_factor` and `estimated_savings_pct` from `Summary`; `bench
summary` reports proxy and reported counts side by side without inventing an
estimate. (Schema/JSONL unaffected — these are derived fields.)

## 7. Objective execution (`eval/code_exec.rs`)

**Extraction.** From an arm's answer text (ZH arm: `chinese_response`, which
carries the untranslated Python; EN arm: `english_control_run.english_response`):
collect fenced ```` ``` ```` blocks (language tag optional); choose the block
containing `def {entry_point}`; else the longest block; else scan raw text for
`def {entry_point}`; else `NoCodeExtracted`.

**Runner.** Per task+arm, in a fresh temp dir, write `runner.py`:

```python
<extracted_code>
<task.test>            # defines check(candidate)
check(<entry_point>)
print("SIGO_OK")
```

Execute `python3 runner.py` with `tokio::process::Command`, `kill_on_drop(true)`,
`stdin` null, a hard wall-clock timeout (default 10 s, configurable), captured
stdout/stderr.

**Outcome classification:**

| Outcome | Condition |
|---|---|
| `Pass` | exit 0 and stdout contains `SIGO_OK` |
| `AssertFail` | non-zero exit, stderr shows `AssertionError` |
| `CompileError` | stderr shows `SyntaxError`/`NameError`/`ImportError` at load |
| `Timeout` | killed by the timeout |
| `RuntimeError` | other non-zero exit |
| `NoCodeExtracted` | extraction found nothing — counts as a **failure** |

`--samples N` generates N answers per arm; pass@1 reported by default, pass@k if
N>1. **An unusable answer is a failure, not a skip** — that is the honest
accounting and is what separates this from a token-only benchmark.

**Safety.** This runs model-generated code on the host. v1 always uses a
throwaway temp dir + timeout + no stdin; if `firejail`/`bwrap` is on PATH it
wraps execution with `--net=none` and a private mount. The report and README
state plainly: *run against untrusted corpora or model output inside a container
or VM.* `doctor` and a preflight check verify `python3` is present and fail fast
otherwise.

## 8. Metrics, cost model, and statistics (`eval/metrics.rs`)

Per task we hold, for each arm: proxy input tokens, authoritative uncached
`input_tokens`, `output_tokens`, `cache_read`, `cache_write`, outcome, latencies;
and for the ZH arm, the round-trip fidelity score.

**Cost model.** New `[pricing]` config (dollars per million tokens):

```toml
[pricing]
input_per_mtok       = 3.0    # static default = Claude Sonnet list price; override per model
output_per_mtok      = 15.0   # ~5× input
cache_read_per_mtok  = 0.30
cache_write_per_mtok = 3.75
```

Defaults are static (Sonnet-tier list prices), not auto-detected from the model;
the user overrides them in `sigo.toml` when benchmarking a different model. Only
the input:output *ratio* affects the cross-arm comparison, so exact numbers
matter mainly for the absolute-dollar columns.

- **Marginal cost** (headline) `= (input·in + output·out) / 1e6` — the fair
  per-prompt bill on `claude-code`, where cached scaffolding is shared overhead.
- **Billed cost** (footnote) adds `cache_read·cr + cache_write·cw`.

**The three layers**, each a paired ZH-vs-EN comparison over tasks:

- **L1 Input cost.** Proxy Δ% and authoritative-uncached-`input_tokens` Δ%, with
  paired bootstrap 95% CI and ZH-win-rate.
- **L2 Dollar cost.** Marginal-cost Δ%, CI, win-rate. Output expansion surfaces
  here.
- **L3 Cost per passing task.** `mean_marginal_cost ÷ pass_rate` per arm; report
  the ZH/EN ratio with a bootstrap CI. Exposes "cheaper per attempt, fails more."

**Statistics (all bundled, no external service):**

- Per-task paired delta `d_i = (ZH_i − EN_i)/EN_i`; report mean and median.
- **Paired bootstrap 95% CI:** resample tasks with replacement, `B = 10_000`,
  seeded by `[benchmark].bootstrap_seed` (default fixed) via a small embedded PCG
  RNG so results are reproducible and unit-testable.
- **ZH-win-rate:** fraction of tasks with ZH cost < EN cost, with a binomial CI.
- **Pass-rate** per arm with a **Wilson 95% CI**.
- **cost-per-pass ratio** ZH/EN with a bootstrap CI (resample tasks, recompute).

## 9. Round-trip fidelity diagnostic (`eval/fidelity.rs`)

For the ZH arm: back-translate the ZH prompt to English via the same Ollama
translator (`ZhToEn`), then ask a local Ollama judge to score closeness of the
back-translation to the original English `prompt` on 0–10 with a short rubric;
parse the integer. Recorded per task (`None` on judge failure — never fails the
task). This is a **diagnostic** that explains *why* ZH pass-rates or costs differ
(e.g., a mangled spec), not a headline metric. Judge model defaults to the
translator model; overridable via config.

## 10. Report

Written to the run directory alongside the existing rolling JSONL.

### `eval_report.md`

- **Header:** run-id, timestamps, wall, backend, claude_model, translator_model,
  corpus source, `--samples`, N attempted/scored/failed-to-run.
- **Headline — three layers:** a table per layer with EN, ZH, Δ% (or ratio),
  **95% CI**, win-rate, and a one-word verdict (`ZH wins`/`EN wins`/`wash`,
  `wash` = CI crosses 0).
- **Correctness:** pass-rate EN vs ZH with Wilson CIs; **cost per passing task**
  EN vs ZH with CI.
- **Failure modes:** counts per arm of `AssertFail`/`CompileError`/`Timeout`/
  `RuntimeError`/`NoCodeExtracted`.
- **Fidelity:** round-trip score distribution (mean, p10/p50/p90).
- **Caveats (auto):** proxy tokenizer is not Claude's; `claude-code` total-input
  is noisy (cache split asymmetric across the paired runs) so the headline uses
  marginal cost; N and CI width; untrusted-code sandbox note; translator
  nondeterminism; `--samples` value.

### `eval_report.csv`

One row per task per arm: `task_id, arm, category, outcome, proxy_in,
reported_in_uncached, output, cache_read, cache_write, marginal_cost,
billed_cost, fidelity_score, translation_in_ms, claude_total_ms, turn_total_ms,
errors`. Plus a `_paired` companion CSV (one row per task with EN/ZH columns and
the per-task deltas) for direct notebook use.

## 11. Error handling

- **Extraction/exec** failures are *data*, not aborts: classified and recorded;
  the run continues.
- **Fidelity judge** failure → `fidelity_score = None`.
- **Translator/Claude** failure → existing behaviour (`errors.jsonl`,
  `incomplete`), and the task is excluded from rate/cost means (reported as
  failed-to-run, not as a correctness failure).
- **`python3` missing** → preflight + `doctor` error before the loop runs.
- The runner never aborts the whole run on a single task.

## 12. Testing

### Unit
- `coding_corpus`: JSONL happy path, malformed line → `line` error, missing field
  → error, bundled subset parses and has 100 entries with non-empty `test`/
  `entry_point`.
- `code_exec::extract`: fenced with/without lang tag, multiple blocks (picks the
  `entry_point` one), prose-wrapped Chinese answer with a Python block, no-code →
  `NoCodeExtracted`.
- `code_exec::run` (requires `python3`, gated behind a `has_python` check): a
  passing solution → `Pass`; a wrong solution → `AssertFail`; `while True: pass`
  → `Timeout`; `def f(:` → `CompileError`.
- `metrics`: cost math; paired bootstrap CI is deterministic under a fixed seed;
  Wilson CI endpoints on known inputs; win-rate.
- `tokenizer::proxy`: `count_tokens("")==0`; English short phrase < Chinese
  equivalent is **not** assumed — instead assert monotonicity and that ZH of a
  known sentence yields a plausible count (guards the o200k wiring, not the
  hypothesis).

### Integration (`tests/eval_coding_e2e.rs`)
- `FakeTranslator` + `FakeBackend` scripted so the EN arm returns a passing
  solution and the ZH arm returns a failing one (and vice versa in a second
  case). Run the eval over a 2-task in-memory corpus to a temp out-dir; assert:
  pass-rates (EN 100% / ZH 0%), the three layer tables exist with finite CIs,
  failure-mode counts, CSV row counts (2 tasks × 2 arms = 4 rows + 2 paired
  rows), `errors.jsonl` absent.

### Live (gated `--features live`)
- Real bundled subset (or `--limit 5`) against real Ollama + the user's
  `claude-code` backend, asserting only that the report is produced and the
  numbers are finite — the experiment itself is run by the user.

## 13. File layout

```
crates/sigo-core/
├── assets/
│   └── humaneval_sample.jsonl        # NEW (100 tasks)
├── Cargo.toml                        # EDIT: -claude-tokenizer +tiktoken-rs (bootstrap RNG = embedded PCG, no new dep)
└── src/
    ├── tokenizer/
    │   ├── proxy.rs                  # NEW (replaces claude.rs)
    │   └── mod.rs                    # EDIT: export TokenizerProxy
    ├── config.rs                     # EDIT: [pricing] block
    ├── benchmark/
    │   ├── coding_corpus.rs          # NEW
    │   ├── summary.rs                # EDIT: remove calibration/estimated savings
    │   └── mod.rs                    # EDIT: re-exports
    └── eval/                         # NEW module
        ├── mod.rs
        ├── code_exec.rs              # extraction + python3 execution
        ├── fidelity.rs               # Ollama round-trip judge
        ├── metrics.rs                # cost model + bootstrap + rates
        └── eval_report.rs            # layered md + csv
crates/sigo-cli/src/commands/
    ├── bench_run.rs                  # EDIT: --eval coding, --samples, post-turn scoring
    └── doctor.rs                     # EDIT: python3 presence check
```

## 14. Success criteria

1. `sigo bench run --eval coding --limit 5` (bundled subset, `claude-code`
   backend) runs end-to-end and writes a readable `eval_report.md` with the three
   layered tables (each with a 95% CI), pass-rates, and cost-per-passing-task.
2. The execution harness scores a known-passing fixture `Pass` and a known-wrong
   fixture `AssertFail`; a non-terminating fixture `Timeout`.
3. Bootstrap CIs are bit-identical across two runs with the same seed.
4. Proxy token counts replace the Claude-2 figures everywhere, labelled as proxy;
   `summary.rs` no longer emits a calibration-based savings estimate.
5. The headline can express a *direction with a confidence interval*, so the
   honest result — Chinese cheaper, costlier, or a wash on Claude — is reported
   with its uncertainty rather than as a point estimate.
6. All offline unit + integration tests pass; the live run produces finite
   numbers against the user's real setup.
```
