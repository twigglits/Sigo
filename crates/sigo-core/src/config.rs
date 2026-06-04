use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Result, SigoError};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SigoConfig {
    #[serde(default)]
    pub translator: TranslatorConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub benchmark: BenchmarkConfig,
    #[serde(default)]
    pub repl: ReplConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatorConfig {
    #[serde(default = "default_translator_provider")]
    pub provider: String,
    #[serde(default = "default_ollama_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_translator_model")]
    pub model: String,
    #[serde(default = "default_translator_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_claude_backend")]
    pub backend: String,
    #[serde(default = "default_claude_model")]
    pub model: String,
    #[serde(default = "default_claude_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub claude_code: ClaudeCodeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeConfig {
    #[serde(default = "default_claude_code_binary")]
    pub binary: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    #[serde(default = "default_control_mode")]
    pub control_mode: String,
    #[serde(default = "default_bootstrap_seed")]
    pub bootstrap_seed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplConfig {
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub history_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    #[serde(default = "default_input_per_mtok")]
    pub input_per_mtok: f64,
    #[serde(default = "default_output_per_mtok")]
    pub output_per_mtok: f64,
    #[serde(default = "default_cache_read_per_mtok")]
    pub cache_read_per_mtok: f64,
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
        Self { binary: default_claude_code_binary(), extra_args: vec![] }
    }
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self { log_path: None, control_mode: default_control_mode(), bootstrap_seed: default_bootstrap_seed() }
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

fn default_bootstrap_seed() -> u64 { 0xC0DE }
fn default_translator_provider() -> String { "ollama".into() }
fn default_ollama_endpoint() -> String { "http://localhost:11434".into() }
fn default_translator_model() -> String { "qwen2.5:7b".into() }
fn default_translator_timeout() -> u64 { 60 }
fn default_claude_backend() -> String { "api".into() }
fn default_claude_model() -> String { "claude-sonnet-4-6".into() }
fn default_claude_max_tokens() -> u32 { 4096 }
fn default_claude_code_binary() -> String { "claude".into() }
fn default_control_mode() -> String { "prompt-only".into() }
fn default_input_per_mtok() -> f64 { 3.0 }
fn default_output_per_mtok() -> f64 { 15.0 }
fn default_cache_read_per_mtok() -> f64 { 0.30 }
fn default_cache_write_per_mtok() -> f64 { 3.75 }

impl SigoConfig {
    /// Load config with precedence: cwd `./sigo.toml` overrides `$XDG_CONFIG_HOME/sigo/config.toml`,
    /// both override built-in defaults. Missing files are not an error.
    pub fn load() -> Result<Self> {
        // Start with built-in defaults serialized to TOML, then layer files via TOML value merge.
        let mut merged: toml::Value = toml::Value::try_from(Self::default())
            .map_err(|e| SigoError::Config(format!("serializing defaults: {e}")))?;

        if let Some(xdg) = xdg_config_path() {
            if xdg.exists() {
                let s = std::fs::read_to_string(&xdg)?;
                let v: toml::Value = toml::from_str(&s)
                    .map_err(|e| SigoError::Config(format!("{}: {e}", xdg.display())))?;
                merge_into(&mut merged, v);
            }
        }
        let cwd_path = PathBuf::from("./sigo.toml");
        if cwd_path.exists() {
            let s = std::fs::read_to_string(&cwd_path)?;
            let v: toml::Value = toml::from_str(&s)
                .map_err(|e| SigoError::Config(format!("{}: {e}", cwd_path.display())))?;
            merge_into(&mut merged, v);
        }

        let mut cfg: SigoConfig = merged
            .try_into()
            .map_err(|e| SigoError::Config(format!("merging: {e}")))?;
        apply_env_overlay(&mut cfg, |k| std::env::var(k).ok())?;
        Ok(cfg)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;
        let mut cfg = parse(&s, path)?;
        apply_env_overlay(&mut cfg, |k| std::env::var(k).ok())?;
        Ok(cfg)
    }

    /// Resolved log path: configured path, else `$XDG_DATA_HOME/sigo/turns.jsonl`.
    pub fn resolved_log_path(&self) -> PathBuf {
        self.benchmark.log_path.clone().unwrap_or_else(|| {
            let data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
            data.join("sigo").join("turns.jsonl")
        })
    }

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
pub fn apply_env_overlay(
    cfg: &mut SigoConfig,
    get: impl Fn(&str) -> Option<String>,
) -> Result<()> {
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
            SigoError::Config(format!("SIGO_CLAUDE_MAX_TOKENS must be a positive integer, got `{v}`"))
        })?;
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

fn parse(s: &str, path: &Path) -> Result<SigoConfig> {
    toml::from_str(s).map_err(|e| SigoError::Config(format!("{}: {e}", path.display())))
}

/// Recursively merge `src` into `dst`. Tables are merged field-by-field; non-table values are
/// replaced. Missing keys in `src` leave `dst` untouched. New keys in `src` are added.
fn merge_into(dst: &mut toml::Value, src: toml::Value) {
    match (dst, src) {
        (toml::Value::Table(dst_t), toml::Value::Table(src_t)) => {
            for (k, v) in src_t {
                match dst_t.get_mut(&k) {
                    Some(existing) => merge_into(existing, v),
                    None => { dst_t.insert(k, v); }
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
        // defaults preserved for unspecified fields
        assert_eq!(c.translator.endpoint, "http://localhost:11434");
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
        let c2: SigoConfig = toml::from_str(r#"
            [pricing]
            input_per_mtok = 15.0
            output_per_mtok = 75.0
        "#).unwrap();
        assert!((c2.pricing.input_per_mtok - 15.0).abs() < 1e-9);
        // unset cache rates keep their defaults
        assert!((c2.pricing.cache_read_per_mtok - 0.30).abs() < 1e-9);
    }

    #[test]
    fn partial_overlay_preserves_unset_fields() {
        let base = toml::Value::try_from(SigoConfig::default()).unwrap();
        let overlay: toml::Value = toml::from_str(r#"
            [translator]
            model = "qwen3:14b"
        "#).unwrap();
        let mut merged = base;
        merge_into(&mut merged, overlay);
        let cfg: SigoConfig = merged.try_into().unwrap();
        assert_eq!(cfg.translator.model, "qwen3:14b");
        // The unset field should still hold the default:
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
        assert_eq!(cfg.benchmark.log_path, Some(PathBuf::from("/data/turns.jsonl")));
    }

    #[test]
    fn env_overlay_overrides_file_values() {
        let mut cfg: SigoConfig = toml::from_str("[translator]\nmodel = \"qwen2.5:3b\"").unwrap();
        apply_env_overlay(&mut cfg, |k| {
            if k == "SIGO_TRANSLATOR_MODEL" { Some("qwen2.5:7b".to_string()) } else { None }
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
            if k == "SIGO_CLAUDE_MAX_TOKENS" { Some("lots".to_string()) } else { None }
        });
        assert!(res.is_err());
    }
}
