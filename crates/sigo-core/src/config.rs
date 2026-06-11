//! Configuration types and loading with layered precedence.
//!
//! Precedence (low → high):
//! 1. Built-in defaults
//! 2. `$XDG_CONFIG_HOME/sigo/config.toml`
//! 3. `./sigo.toml` (or `--config <path>`)
//! 4. `SIGO_*` environment variables
//! 5. CLI flags (applied by the CLI layer, not here)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Result, SigoError};

/// Top-level configuration. All sub-configs have [`Default`] so partial TOML
/// files are merged over the built-in defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SigoConfig {
    /// Translator (Ollama) settings.
    #[serde(default)]
    pub translator: TranslatorConfig,
    /// Claude backend settings.
    #[serde(default)]
    pub claude: ClaudeConfig,
    /// Benchmark control and logging.
    #[serde(default)]
    pub benchmark: BenchmarkConfig,
    /// REPL behaviour.
    #[serde(default)]
    pub repl: ReplConfig,
    /// Dollar-per-million-token rates for cost computation.
    #[serde(default)]
    pub pricing: PricingConfig,
}

/// Local translator (Ollama) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatorConfig {
    /// Provider name (currently only `"ollama"`).
    #[serde(default = "default_translator_provider")]
    pub provider: String,
    /// Ollama API endpoint, e.g. `"http://localhost:11434"`.
    #[serde(default = "default_ollama_endpoint")]
    pub endpoint: String,
    /// Model name, e.g. `"qwen2.5:7b"`.
    #[serde(default = "default_translator_model")]
    pub model: String,
    /// Per-request timeout in seconds.
    #[serde(default = "default_translator_timeout")]
    pub timeout_seconds: u64,
    /// Translation style (terse minimizes tokens; fluent is the baseline).
    #[serde(default)]
    pub style: TranslatorStyle,
}

/// Register requested from the EN→ZH translator. Unknown values are rejected
/// at parse time (serde enum), matching the CLI's ValueEnum convention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslatorStyle {
    /// Maximally concise written Chinese — the token-minimizing default.
    #[default]
    Terse,
    /// Natural, fluent translation — kept so paired benchmark runs can
    /// attribute savings to the register rather than to translation per se.
    Fluent,
}

impl TranslatorStyle {
    /// Canonical lowercase name, matching the TOML/env spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terse => "terse",
            Self::Fluent => "fluent",
        }
    }
}

/// Claude backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    /// Backend kind: `"api"` or `"claude-code"`.
    #[serde(default = "default_claude_backend")]
    pub backend: String,
    /// Model name, e.g. `"claude-sonnet-4-6"`.
    #[serde(default = "default_claude_model")]
    pub model: String,
    /// Maximum output tokens per turn.
    #[serde(default = "default_claude_max_tokens")]
    pub max_tokens: u32,
    /// Claude Code CLI-specific settings.
    #[serde(default)]
    pub claude_code: ClaudeCodeConfig,
}

/// Claude Code CLI sub-process configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeConfig {
    /// Path or name of the `claude` CLI binary.
    #[serde(default = "default_claude_code_binary")]
    pub binary: String,
    /// Extra CLI arguments passed on every invocation.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// Benchmark logging and control-arm configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    /// Override for the JSONL log path. Defaults to `$XDG_DATA_HOME/sigo/turns.jsonl`.
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    /// Control mode: `"off"`, `"prompt-only"`, or `"full"`.
    #[serde(default = "default_control_mode")]
    pub control_mode: String,
    /// Seed for bootstrap CI reproducibility in eval reports.
    #[serde(default = "default_bootstrap_seed")]
    pub bootstrap_seed: u64,
}

/// REPL behaviour.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplConfig {
    /// Show verbose turn footers (ZH bridge + token panel).
    #[serde(default)]
    pub verbose: bool,
    /// Override for the readline history file path.
    #[serde(default)]
    pub history_file: Option<PathBuf>,
}

/// Dollar-per-million-token rates for cost computation.
///
/// Defaults match Sonnet list price; override for other models or negotiated rates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// $/M input tokens.
    #[serde(default = "default_input_per_mtok")]
    pub input_per_mtok: f64,
    /// $/M output tokens.
    #[serde(default = "default_output_per_mtok")]
    pub output_per_mtok: f64,
    /// $/M cache read tokens.
    #[serde(default = "default_cache_read_per_mtok")]
    pub cache_read_per_mtok: f64,
    /// $/M cache write tokens.
    #[serde(default = "default_cache_write_per_mtok")]
    pub cache_write_per_mtok: f64,
}

impl Default for TranslatorConfig {
    fn default() -> Self {
        Self {
            provider: default_translator_provider(),
            endpoint: default_ollama_endpoint(),
            model: default_translator_model(),
            timeout_seconds: default_translator_timeout(),
            style: TranslatorStyle::default(),
        }
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            backend: default_claude_backend(),
            model: default_claude_model(),
            max_tokens: default_claude_max_tokens(),
            claude_code: ClaudeCodeConfig::default(),
        }
    }
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            binary: default_claude_code_binary(),
            extra_args: vec![],
        }
    }
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            log_path: None,
            control_mode: default_control_mode(),
            bootstrap_seed: default_bootstrap_seed(),
        }
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            input_per_mtok: default_input_per_mtok(),
            output_per_mtok: default_output_per_mtok(),
            cache_read_per_mtok: default_cache_read_per_mtok(),
            cache_write_per_mtok: default_cache_write_per_mtok(),
        }
    }
}

fn default_bootstrap_seed() -> u64 {
    0xC0DE
}
fn default_translator_provider() -> String {
    "ollama".into()
}
fn default_ollama_endpoint() -> String {
    "http://localhost:11434".into()
}
fn default_translator_model() -> String {
    "qwen2.5:7b".into()
}
fn default_translator_timeout() -> u64 {
    60
}
fn default_claude_backend() -> String {
    "api".into()
}
fn default_claude_model() -> String {
    "claude-sonnet-4-6".into()
}
fn default_claude_max_tokens() -> u32 {
    4096
}
fn default_claude_code_binary() -> String {
    "claude".into()
}
fn default_control_mode() -> String {
    "prompt-only".into()
}
fn default_input_per_mtok() -> f64 {
    3.0
}
fn default_output_per_mtok() -> f64 {
    15.0
}
fn default_cache_read_per_mtok() -> f64 {
    0.30
}
fn default_cache_write_per_mtok() -> f64 {
    3.75
}

impl SigoConfig {
    /// Load config with precedence: cwd `./sigo.toml` overrides `$XDG_CONFIG_HOME/sigo/config.toml`,
    /// both override built-in defaults. Missing files are not an error.
    pub fn load() -> Result<Self> {
        let mut layers = Vec::new();
        if let Some(xdg) = xdg_config_path() {
            if xdg.exists() {
                layers.push(read_toml_layer(&xdg)?);
            }
        }
        let cwd_path = PathBuf::from("./sigo.toml");
        if cwd_path.exists() {
            layers.push(read_toml_layer(&cwd_path)?);
        }
        layered_config(layers, |k| std::env::var(k).ok())
    }

    /// Same layering as [`SigoConfig::load`], but an explicit `--config <path>` substitutes
    /// for the cwd `./sigo.toml` layer: defaults < XDG `config.toml` < `path` < `SIGO_*` env.
    /// The named file is required (a missing path errors); the XDG base is still applied
    /// underneath, so `--config` no longer silently discards it.
    pub fn load_from(path: &Path) -> Result<Self> {
        let mut layers = Vec::new();
        if let Some(xdg) = xdg_config_path() {
            if xdg.exists() {
                layers.push(read_toml_layer(&xdg)?);
            }
        }
        layers.push(read_toml_layer(path)?);
        layered_config(layers, |k| std::env::var(k).ok())
    }

    /// Resolved log path: configured path, else `$XDG_DATA_HOME/sigo/turns.jsonl`.
    pub fn resolved_log_path(&self) -> PathBuf {
        self.benchmark.log_path.clone().unwrap_or_else(|| {
            let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            data.join("sigo").join("turns.jsonl")
        })
    }

    /// Resolved readline history file path.
    pub fn resolved_history_path(&self) -> PathBuf {
        self.repl.history_file.clone().unwrap_or_else(|| {
            let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            data.join("sigo").join("history")
        })
    }
}

/// Overlay recognized `SIGO_*` environment variables onto a config. `get` resolves a var
/// name to its value (inject a fake map in tests; production passes `std::env::var(k).ok()`).
/// Applied after file merge and before CLI flags, so env beats files but flags beat env.
pub fn apply_env_overlay(cfg: &mut SigoConfig, get: impl Fn(&str) -> Option<String>) -> Result<()> {
    if let Some(v) = get("SIGO_TRANSLATOR_ENDPOINT") {
        cfg.translator.endpoint = v;
    }
    if let Some(v) = get("SIGO_TRANSLATOR_MODEL") {
        cfg.translator.model = v;
    }
    if let Some(v) = get("SIGO_CLAUDE_BACKEND") {
        cfg.claude.backend = v;
    }
    if let Some(v) = get("SIGO_CLAUDE_MODEL") {
        cfg.claude.model = v;
    }
    if let Some(v) = get("SIGO_CLAUDE_MAX_TOKENS") {
        cfg.claude.max_tokens = v.parse().map_err(|_| {
            SigoError::Config(format!(
                "SIGO_CLAUDE_MAX_TOKENS must be a positive integer, got `{v}`"
            ))
        })?;
    }
    if let Some(v) = get("SIGO_TRANSLATOR_STYLE") {
        cfg.translator.style = match v.as_str() {
            "terse" => TranslatorStyle::Terse,
            "fluent" => TranslatorStyle::Fluent,
            _ => {
                return Err(SigoError::Config(format!(
                    "SIGO_TRANSLATOR_STYLE must be `terse` or `fluent`, got `{v}`"
                )))
            }
        };
    }
    if let Some(v) = get("SIGO_CONTROL_MODE") {
        cfg.benchmark.control_mode = v;
    }
    if let Some(v) = get("SIGO_LOG_PATH") {
        cfg.benchmark.log_path = Some(PathBuf::from(v));
    }
    Ok(())
}

fn xdg_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("sigo").join("config.toml"))
}

fn read_toml_layer(path: &Path) -> Result<toml::Value> {
    let s = std::fs::read_to_string(path)?;
    toml::from_str(&s).map_err(|e| SigoError::Config(format!("{}: {e}", path.display())))
}

/// Merge `layers` (low → high) over the built-in defaults, then apply the `SIGO_*` env
/// overlay (which beats every file). Shared by `load` and `load_from` so the precedence is
/// identical regardless of how the top file layer was chosen.
fn layered_config(
    layers: Vec<toml::Value>,
    get_env: impl Fn(&str) -> Option<String>,
) -> Result<SigoConfig> {
    let mut merged: toml::Value = toml::Value::try_from(SigoConfig::default())
        .map_err(|e| SigoError::Config(format!("serializing defaults: {e}")))?;
    for v in layers {
        merge_into(&mut merged, v);
    }
    let mut cfg: SigoConfig = merged
        .try_into()
        .map_err(|e| SigoError::Config(format!("merging config: {e}")))?;
    apply_env_overlay(&mut cfg, get_env)?;
    Ok(cfg)
}

/// Recursively merge `src` into `dst`. Tables are merged field-by-field; non-table values are
/// replaced. Missing keys in `src` leave `dst` untouched. New keys in `src` are added.
fn merge_into(dst: &mut toml::Value, src: toml::Value) {
    match (dst, src) {
        (toml::Value::Table(dst_t), toml::Value::Table(src_t)) => {
            for (k, v) in src_t {
                match dst_t.get_mut(&k) {
                    Some(existing) => merge_into(existing, v),
                    None => {
                        dst_t.insert(k, v);
                    }
                }
            }
        }
        (dst_slot, src_v) => {
            *dst_slot = src_v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_cleanly() {
        let c = SigoConfig::default();
        assert_eq!(c.translator.provider, "ollama");
        assert_eq!(c.claude.backend, "api");
    }

    #[test]
    fn parses_partial_toml() {
        let toml = r#"
            [translator]
            model = "qwen3:14b"
            [claude]
            backend = "claude-code"
        "#;
        let c: SigoConfig = toml::from_str(toml).unwrap();
        assert_eq!(c.translator.model, "qwen3:14b");
        assert_eq!(c.claude.backend, "claude-code");
        assert_eq!(c.translator.endpoint, "http://localhost:11434");
    }

    #[test]
    fn translator_style_defaults_to_terse() {
        assert_eq!(
            SigoConfig::default().translator.style,
            TranslatorStyle::Terse
        );
        let parsed: SigoConfig = toml::from_str("[translator]\nmodel = \"qwen3:14b\"").unwrap();
        assert_eq!(parsed.translator.style, TranslatorStyle::Terse);
    }

    #[test]
    fn translator_style_parses_fluent_and_rejects_unknown() {
        let c: SigoConfig = toml::from_str("[translator]\nstyle = \"fluent\"").unwrap();
        assert_eq!(c.translator.style, TranslatorStyle::Fluent);
        assert!(
            toml::from_str::<SigoConfig>("[translator]\nstyle = \"verbose\"").is_err(),
            "unknown style must be rejected at parse time"
        );
    }

    #[test]
    fn env_overlay_sets_translator_style_and_rejects_garbage() {
        let mut cfg = SigoConfig::default();
        apply_env_overlay(&mut cfg, |k| {
            (k == "SIGO_TRANSLATOR_STYLE").then(|| "fluent".to_string())
        })
        .unwrap();
        assert_eq!(cfg.translator.style, TranslatorStyle::Fluent);

        let mut cfg = SigoConfig::default();
        let res = apply_env_overlay(&mut cfg, |k| {
            (k == "SIGO_TRANSLATOR_STYLE").then(|| "verbose".to_string())
        });
        assert!(res.is_err(), "garbage SIGO_TRANSLATOR_STYLE must error");
    }

    #[test]
    fn resolved_log_path_uses_xdg_when_unset() {
        let c = SigoConfig::default();
        let path = c.resolved_log_path();
        assert!(path.ends_with("sigo/turns.jsonl"));
    }

    #[test]
    fn pricing_defaults_and_override() {
        let c = SigoConfig::default();
        assert!((c.pricing.input_per_mtok - 3.0).abs() < 1e-9);
        assert!((c.pricing.output_per_mtok - 15.0).abs() < 1e-9);
        let c2: SigoConfig = toml::from_str(
            r#"
            [pricing]
            input_per_mtok = 15.0
            output_per_mtok = 75.0
        "#,
        )
        .unwrap();
        assert!((c2.pricing.input_per_mtok - 15.0).abs() < 1e-9);
        assert!((c2.pricing.cache_read_per_mtok - 0.30).abs() < 1e-9);
    }

    #[test]
    fn partial_overlay_preserves_unset_fields() {
        let base = toml::Value::try_from(SigoConfig::default()).unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [translator]
            model = "qwen3:14b"
        "#,
        )
        .unwrap();
        let mut merged = base;
        merge_into(&mut merged, overlay);
        let cfg: SigoConfig = merged.try_into().unwrap();
        assert_eq!(cfg.translator.model, "qwen3:14b");
        assert_eq!(cfg.claude.backend, "api");
        assert_eq!(cfg.translator.endpoint, "http://localhost:11434");
    }

    #[test]
    fn env_overlay_sets_fields() {
        use std::collections::HashMap;
        let mut cfg = SigoConfig::default();
        let env: HashMap<&str, &str> = [
            ("SIGO_TRANSLATOR_ENDPOINT", "http://ollama:11434"),
            ("SIGO_CLAUDE_BACKEND", "claude-code"),
            ("SIGO_CLAUDE_MAX_TOKENS", "8192"),
            ("SIGO_LOG_PATH", "/data/turns.jsonl"),
        ]
        .into_iter()
        .collect();
        apply_env_overlay(&mut cfg, |k| env.get(k).map(|s| s.to_string())).unwrap();
        assert_eq!(cfg.translator.endpoint, "http://ollama:11434");
        assert_eq!(cfg.claude.backend, "claude-code");
        assert_eq!(cfg.claude.max_tokens, 8192);
        assert_eq!(
            cfg.benchmark.log_path,
            Some(PathBuf::from("/data/turns.jsonl"))
        );
    }

    #[test]
    fn layered_config_applies_defaults_xdg_top_env_in_order() {
        let xdg: toml::Value = toml::from_str(
            "[claude]\nbackend = \"claude-code\"\n[translator]\nmodel = \"from-xdg\"",
        )
        .unwrap();
        let top: toml::Value = toml::from_str("[translator]\nmodel = \"from-top\"").unwrap();
        let env = |k: &str| (k == "SIGO_CLAUDE_MODEL").then(|| "from-env".to_string());
        let cfg = layered_config(vec![xdg, top], env).unwrap();
        assert_eq!(cfg.claude.backend, "claude-code");
        assert_eq!(cfg.translator.model, "from-top");
        assert_eq!(cfg.claude.model, "from-env");
        assert_eq!(cfg.translator.endpoint, "http://localhost:11434");
    }

    #[test]
    fn env_overlay_overrides_file_values() {
        let mut cfg: SigoConfig = toml::from_str("[translator]\nmodel = \"qwen2.5:3b\"").unwrap();
        apply_env_overlay(&mut cfg, |k| {
            if k == "SIGO_TRANSLATOR_MODEL" {
                Some("qwen2.5:7b".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(cfg.translator.model, "qwen2.5:7b");
    }

    #[test]
    fn env_overlay_unset_leaves_defaults() {
        let mut cfg = SigoConfig::default();
        apply_env_overlay(&mut cfg, |_| None).unwrap();
        assert_eq!(cfg.translator.model, "qwen2.5:7b");
        assert_eq!(cfg.claude.backend, "api");
    }

    #[test]
    fn env_overlay_bad_max_tokens_errors() {
        let mut cfg = SigoConfig::default();
        let res = apply_env_overlay(&mut cfg, |k| {
            if k == "SIGO_CLAUDE_MAX_TOKENS" {
                Some("lots".to_string())
            } else {
                None
            }
        });
        assert!(res.is_err());
    }
}
