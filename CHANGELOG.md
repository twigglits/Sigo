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

### Changed
- Module-level doc comments expanded on `claude`, `benchmark`, `tokenizer`, `stream`, `eval`
