//! One-shot `sigo chat`: run a single turn non-interactively.

use anyhow::Result;
use sigo_core::{Orchestrator, OutputSink, SigoConfig, StdoutSink};

/// Resolve the prompt: use `arg` if non-empty, else read all of stdin (trimmed).
pub fn resolve_prompt(arg: Option<String>) -> Result<String> {
    if let Some(p) = arg {
        if p.trim().is_empty() {
            anyhow::bail!("empty prompt argument");
        }
        return Ok(p);
    }
    use std::io::Read;
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| anyhow::anyhow!("reading stdin: {e}"))?;
    let t = s.trim();
    if t.is_empty() {
        anyhow::bail!("no prompt: pass an argument or pipe text on stdin");
    }
    Ok(t.to_string())
}

/// Run exactly one turn through `orch`, writing the English answer to `out`.
/// With `verbose`, prints a one-line summary to stderr. Errors on an incomplete turn.
pub async fn run_once(
    orch: &mut Orchestrator,
    prompt: &str,
    out: &mut dyn OutputSink,
    verbose: bool,
) -> Result<()> {
    let record = orch.run_turn(prompt, out).await?;
    out.write("\n");
    out.flush();
    if verbose {
        let zh_in = record
            .chinese_prompt_tokens_reported
            .map(|x| x.to_string())
            .unwrap_or_else(|| "—".to_string());
        eprintln!(
            "[turn {} · {} ms · ZH-in {} reported vs EN-proxy {} local]",
            record.turn_index, record.turn_total_ms, zh_in, record.english_prompt_tokens_local
        );
    }
    if record.incomplete {
        anyhow::bail!(
            "turn incomplete — translator/backend failed: {}",
            record.turn_errors.join("; ")
        );
    }
    Ok(())
}

/// Top-level entry: resolve the prompt, preflight the translator, build the stack, run one turn.
pub async fn run(config: &SigoConfig, prompt_arg: Option<String>, verbose: bool) -> Result<()> {
    let prompt = resolve_prompt(prompt_arg)?;
    super::checks::preflight_translator(config).await?;
    let mut orch = crate::repl::build_orchestrator(config)?;
    let mut out = StdoutSink;
    run_once(&mut orch, &prompt, &mut out, verbose).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_prompt_is_used_verbatim() {
        let p = resolve_prompt(Some("explain ownership".to_string())).unwrap();
        assert_eq!(p, "explain ownership");
    }

    #[test]
    fn empty_arg_is_rejected() {
        assert!(resolve_prompt(Some("   ".to_string())).is_err());
    }
}
