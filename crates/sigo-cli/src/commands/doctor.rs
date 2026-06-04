use anyhow::Result;
use serde::Deserialize;
use sigo_core::{SigoConfig, TokenizerProxy, Tokenizer};
use std::time::Duration;

pub async fn run(config: &SigoConfig) -> Result<()> {
    let mut all_ok = true;

    println!("== config ==");
    println!("translator: {} @ {}", config.translator.model, config.translator.endpoint);
    println!("claude:     {} (backend={})", config.claude.model, config.claude.backend);
    println!("log path:   {}", config.resolved_log_path().display());

    println!("\n== checks ==");

    all_ok &= check(
        "ollama reachable",
        check_ollama_reachable(&config.translator.endpoint).await,
    );
    all_ok &= check(
        "translator model present",
        check_ollama_model(&config.translator.endpoint, &config.translator.model).await,
    );

    match config.claude.backend.as_str() {
        "api" => {
            all_ok &= check("ANTHROPIC_API_KEY set", check_api_key().await);
        }
        "claude-code" => {
            all_ok &= check(
                "`claude` binary available",
                check_claude_binary(&config.claude.claude_code.binary).await,
            );
        }
        other => {
            println!("[FAIL] unknown backend `{other}`");
            all_ok = false;
        }
    }

    all_ok &= check("tokenizer loadable", check_tokenizer().await);
    all_ok &= check("python3 available (for --eval coding)", check_python3().await);
    all_ok &= check(
        "log path writable",
        check_log_writable(&config.resolved_log_path()).await,
    );

    println!();
    if all_ok {
        println!("doctor: OK");
        Ok(())
    } else {
        println!("doctor: one or more checks failed");
        std::process::exit(1)
    }
}

fn check(label: &str, res: Result<String>) -> bool {
    match res {
        Ok(msg) => {
            println!("[ OK ] {label}: {msg}");
            true
        }
        Err(e) => {
            println!("[FAIL] {label}: {e}");
            false
        }
    }
}

async fn check_ollama_reachable(endpoint: &str) -> Result<String> {
    let url = format!("{}/api/version", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let resp = client.get(&url).send().await.map_err(|e| {
        anyhow::anyhow!("can't reach {url}: {e} — is `ollama serve` running?")
    })?;
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

async fn check_ollama_model(endpoint: &str, model: &str) -> Result<String> {
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

async fn check_api_key() -> Result<String> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow::anyhow!("ANTHROPIC_API_KEY env var missing"))?;
    if key.len() < 8 {
        anyhow::bail!("ANTHROPIC_API_KEY looks too short");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = serde_json::json!({
        "model": "claude-haiku-4-5",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = client.post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Anthropic ping failed: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(format!("env var set ({} chars), ping OK", key.len()))
    } else if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        anyhow::bail!("API responded {status} — key may be invalid or revoked")
    } else {
        // Other errors (e.g. 400 from a model name change) — surface as a soft warning, not a hard fail.
        Ok(format!("env var set ({} chars); Anthropic returned {} (auth still considered valid)", key.len(), status))
    }
}

async fn check_claude_binary(binary: &str) -> Result<String> {
    let out = tokio::process::Command::new(binary)
        .arg("--version")
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawn `{binary}`: {e}"))?;
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(v)
    } else {
        anyhow::bail!(
            "exit {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

async fn check_tokenizer() -> Result<String> {
    let t = TokenizerProxy::new()?;
    let n = t.count_tokens("hello world")?;
    Ok(format!("{} loaded, sample count = {n}", TokenizerProxy::label()))
}

async fn check_python3() -> Result<String> {
    let out = tokio::process::Command::new("python3")
        .arg("--version")
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawn python3: {e} — install Python 3 to use `--eval coding`"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        anyhow::bail!("python3 --version exited {}", out.status)
    }
}

async fn check_log_writable(path: &std::path::Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(path.display().to_string())
}
