# Sigo dev tasks — run with `just <recipe>`. Install just: https://github.com/casey/just

# List available recipes
default:
    @just --list

# Build the whole workspace (debug)
build:
    cargo build --workspace

# Build the release binary (target/release/sigo)
release:
    cargo build --release -p sigo-cli

# Run the test suite
test:
    cargo test --workspace

# Auto-format the code
fmt:
    cargo fmt --all

# Lint exactly as CI does: format check + clippy with warnings denied
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Live tests against real Ollama + Anthropic API (needs creds; off by default)
test-live:
    cargo test -p sigo-core --features live

# Run the CLI/REPL, e.g. `just run doctor` or `just run chat "hi"`
run *ARGS:
    cargo run -p sigo-cli -- {{ARGS}}

# Verify local setup (Ollama, model, Claude auth, tokenizer, python3)
doctor:
    cargo run -p sigo-cli -- doctor

# Smoke-run the objective coding benchmark over the first 5 tasks
bench-coding:
    cargo run -p sigo-cli -- bench run --eval coding --limit 5

# Build the Docker image locally
docker:
    docker build -t sigo:dev .

# Check the workspace against the declared MSRV (matches CI)
msrv:
    cargo +1.88.0 check --workspace --all-targets --locked
