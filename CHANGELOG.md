# Changelog

All notable changes to Sigo are tracked here. This project follows
[Semantic Versioning](https://semver.org/) with automatic releases derived from
[conventional commits](https://www.conventionalcommits.org/). See the
[releases page](https://github.com/twigglits/Sigo/releases) for full release notes.

## [Unreleased]

### Added
- Issue templates, PR template, CONTRIBUTING.md, SECURITY.md, CHANGELOG.md
- `#![warn(missing_docs)]` on both crates with doc-comments added for all public items
- Doc-tests on key public APIs (`compact_zh`, `TokenizerProxy`, `Conversation`)
- Repository metadata (description, homepage, repository, keywords, categories) in Cargo.toml
- `AnyTranslator` / `AnyClaudeBackend` enum dispatch — replaces `Arc<dyn Trait>`, enables native `async fn` (RPITIT)
- `/clear` slash-command to purge session without `/reset`
- Cross-platform Python sandbox preamble (null socket/urllib/ctypes/ffi/asyncio) + macOS `sandbox-exec` fallback
- Input sanitization (`translator/sanitize.rs`) strips null/control chars and neuters `<source>` markers
- Structured tracing (`#[instrument]`, `info_span!`) on core pipeline phases
- `ClaudeConfig` fields `temperature`/`top_p` with env var overrides
- Token-regression CI test with snapshot updater

### Changed
- Module-level doc comments expanded on `claude`, `benchmark`, `tokenizer`, `stream`, `eval`
- Translator and Claude-backend traits use native `async fn` — removed `#[async_trait]` and `async-trait` dependency
- Backend hot-swap (`/model`, `/backend`) now assigns enum variants instead of `Arc<dyn Trait>`
- `translator_builder`/`backend_builder` types return `AnyTranslator`/`AnyClaudeBackend`

### Fixed
- Security advisory: `indicatif` 0.17→0.18 removed `number_prefix` vulnerability
- CI formatting drift: `cargo fmt --all` applied across workspace
