# Contributing to Sigo

## Setup

```bash
git clone https://github.com/twigglits/Sigo && cd Sigo
cp .env.example .env               # add your ANTHROPIC_API_KEY
cargo build --workspace
git config core.hooksPath .githooks  # install pre-commit formatting hook
```

## Development workflow

We use [just](https://github.com/casey/just) as a task runner:

```bash
just test        # full test suite
just lint        # fmt + clippy exactly as CI
just build       # debug build
just release     # release binary
just bench-coding  # smoke-run the coding benchmark
```

Or use `cargo` directly:

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Project structure

- `crates/sigo-core/` — library: traits, orchestrator, translator, tokenizer, benchmark sinks, eval
- `crates/sigo-cli/` — binary: clap CLI, rustyline REPL, subcommands (chat, doctor, bench, config-show)

## Before committing

1. **Tests pass.** `cargo test --workspace` must succeed.
2. **Formatting.** `cargo fmt --all` applied.
3. **Clippy clean.** `cargo clippy --workspace --all-targets -- -D warnings` — no warnings.
4. **MSRV.** `cargo check --workspace --all-targets --locked` on the declared MSRV (1.88).
5. **Doc warnings.** `cargo doc --workspace --no-deps` produces no warnings.

## Conventional commits

This repo uses [conventional commits](https://www.conventionalcommits.org/) for automatic versioning:

| Prefix     | Effect on 0.x | Effect on 1.x+ |
|------------|---------------|-----------------|
| `feat:`    | minor bump    | minor bump      |
| `fix:`     | patch bump    | patch bump      |
| `perf:`    | patch bump    | patch bump      |
| `revert:`  | patch bump    | patch bump      |
| `docs:`    | no release    | no release      |
| `chore:`   | no release    | no release      |
| `ci:`      | no release    | no release      |
| `refactor:`| no release    | no release      |
| `test:`    | no release    | no release      |
| `BREAKING CHANGE` footer | major bump | major bump |

## Live tests

Tests that hit a real Ollama or Anthropic API are gated behind `--features live`:

```bash
cargo test -p sigo-core --features live
```

These are not run by default in CI. Ensure `ollama pull qwen2.5:7b` has been run before using live tests.

## Adding a dependency

New dependencies must be on [crates.io](https://crates.io) (no git/path sources). If the
license isn't already in `deny.toml`'s allow-list, add it after review. Run `cargo deny check`
locally before committing.

## Release process

Releases are automatic: push to `main` triggers the Version workflow, which derives the
increment from conventional commits, bumps workspace versions, tags, and publishes to
GitHub Releases + GHCR. Manual dispatch via Actions → Version → Run workflow supports
explicit `patch`/`minor`/`major` overrides.
