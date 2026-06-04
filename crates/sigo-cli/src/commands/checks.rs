//! Shared connectivity probes for the Ollama translator, used by `doctor` and the
//! startup preflight. Kept here so there is one source of truth for the checks.

use anyhow::Result;
use serde::Deserialize;
use sigo_core::SigoConfig;
use std::time::Duration;

/// Probe `GET {endpoint}/api/version`. Ok with the HTTP status string when reachable.
pub async fn ollama_reachable(endpoint: &str) -> Result<String> {
    let url = format!("{}/api/version", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("can't reach {url}: {e}"))?;
    Ok(format!("HTTP {}", resp.status()))
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

/// Probe `GET {endpoint}/api/tags` and confirm `model` is installed.
pub async fn ollama_has_model(endpoint: &str, model: &str) -> Result<String> {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let resp = client.get(&url).send().await?;
    let tags: TagsResponse = resp.json().await?;
    if tags.models.iter().any(|m| m.name == model) {
        Ok(format!("found `{model}`"))
    } else {
        anyhow::bail!("model `{model}` not installed — run `ollama pull {model}`")
    }
}

/// Fail fast before the first turn if the translator can't serve. Returns an error whose
/// message tells the user exactly how to fix it (hard-fail, no silent passthrough).
pub async fn preflight_translator(cfg: &SigoConfig) -> Result<()> {
    ollama_reachable(&cfg.translator.endpoint).await.map_err(|e| {
        anyhow::anyhow!(
            "translator unavailable: {e}\n  fix: start Ollama with `ollama serve`, or set SIGO_TRANSLATOR_ENDPOINT to a reachable instance"
        )
    })?;
    ollama_has_model(&cfg.translator.endpoint, &cfg.translator.model)
        .await
        .map_err(|e| anyhow::anyhow!("translator: {e}"))?;
    Ok(())
}
