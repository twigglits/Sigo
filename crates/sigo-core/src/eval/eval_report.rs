use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::config::PricingConfig;
use crate::eval::code_exec::Outcome;
use crate::eval::metrics::{bootstrap_ci_mean, pct_delta, win_rate, wilson_ci, ArmCost, Pcg32};

/// One arm's result for one task.
#[derive(Debug, Clone, Copy)]
pub struct ArmEval {
    pub outcome: Outcome,
    pub cost: ArmCost,
    pub proxy_in: u32,
}

/// Paired EN/ZH result for one task.
#[derive(Debug, Clone)]
pub struct TaskEval {
    pub task_id: String,
    pub category: String,
    pub en: ArmEval,
    pub zh: ArmEval,
    pub fidelity: Option<u8>,
}

/// A layer's paired comparison (mean delta % with bootstrap CI + win-rate).
#[derive(Debug, Clone)]
pub struct LayerStat {
    pub mean_en: f64,
    pub mean_zh: f64,
    pub mean_delta_pct: f64,
    pub ci_lo: f64,
    pub ci_hi: f64,
    pub win_rate: f64,
}

#[derive(Debug, Clone)]
pub struct PassStat {
    pub passes: usize,
    pub n: usize,
    pub rate: f64,
    pub ci_lo: f64,
    pub ci_hi: f64,
}

#[derive(Debug, Clone)]
pub struct EvalSummary {
    pub n: usize,
    pub proxy_input: LayerStat,
    pub reported_input: LayerStat,
    pub marginal_dollars: LayerStat,
    pub en_pass: PassStat,
    pub zh_pass: PassStat,
    pub en_cost_per_pass: f64,
    pub zh_cost_per_pass: f64,
    pub cost_per_pass_ratio: f64,       // ZH cost-per-pass / EN cost-per-pass
    pub cost_per_pass_ci: (f64, f64),   // bootstrap 95% CI of that ratio
    pub failure_modes_en: BTreeMap<String, usize>,
    pub failure_modes_zh: BTreeMap<String, usize>,
    pub fidelity_mean: Option<f64>,
}

fn layer(en: &[f64], zh: &[f64], deltas: &[f64], seed: u64) -> LayerStat {
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let (ci_lo, ci_hi) = bootstrap_ci_mean(deltas, 10_000, seed);
    LayerStat {
        mean_en: mean(en),
        mean_zh: mean(zh),
        mean_delta_pct: mean(deltas),
        ci_lo,
        ci_hi,
        win_rate: win_rate(deltas),
    }
}

fn pass_stat(arm: impl Fn(&TaskEval) -> &ArmEval, tasks: &[TaskEval]) -> PassStat {
    let n = tasks.len();
    let passes = tasks.iter().filter(|t| arm(t).outcome.is_pass()).count();
    let (lo, hi) = wilson_ci(passes, n);
    PassStat { passes, n, rate: if n == 0 { 0.0 } else { passes as f64 / n as f64 }, ci_lo: lo, ci_hi: hi }
}

fn modes(arm: impl Fn(&TaskEval) -> &ArmEval, tasks: &[TaskEval]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for t in tasks {
        let a = arm(t);
        if !a.outcome.is_pass() {
            *m.entry(a.outcome.label().to_string()).or_insert(0) += 1;
        }
    }
    m
}

/// Bootstrap the ZH/EN cost-per-passing-task ratio by resampling tasks. Resamples
/// that yield zero passes in either arm are skipped (ratio undefined there).
fn cost_per_pass_ratio_ci(tasks: &[TaskEval], pricing: &PricingConfig, b: usize, seed: u64) -> (f64, (f64, f64)) {
    let arm_cpp = |sel: fn(&TaskEval) -> &ArmEval, idxs: &[usize]| -> Option<f64> {
        let mut sum = 0.0;
        let mut passes = 0usize;
        for &i in idxs {
            let a = sel(&tasks[i]);
            sum += a.cost.marginal(pricing);
            if a.outcome.is_pass() { passes += 1; }
        }
        if passes == 0 { None } else { Some(sum / passes as f64) }
    };
    let all: Vec<usize> = (0..tasks.len()).collect();
    let point = match (arm_cpp(|t| &t.zh, &all), arm_cpp(|t| &t.en, &all)) {
        (Some(z), Some(e)) if e > 0.0 => z / e,
        _ => f64::NAN,
    };
    let mut rng = Pcg32::new(seed);
    let mut ratios: Vec<f64> = Vec::new();
    for _ in 0..b {
        let idxs: Vec<usize> = (0..tasks.len()).map(|_| rng.below(tasks.len())).collect();
        if let (Some(z), Some(e)) = (arm_cpp(|t| &t.zh, &idxs), arm_cpp(|t| &t.en, &idxs)) {
            if e > 0.0 { ratios.push(z / e); }
        }
    }
    if ratios.is_empty() { return (point, (f64::NAN, f64::NAN)); }
    ratios.sort_by(|a, c| a.partial_cmp(c).unwrap());
    let lo = ratios[((0.025 * ratios.len() as f64) as usize).min(ratios.len() - 1)];
    let hi = ratios[((0.975 * ratios.len() as f64) as usize).min(ratios.len() - 1)];
    (point, (lo, hi))
}

pub fn summarize_eval(tasks: &[TaskEval], pricing: &PricingConfig, seed: u64) -> EvalSummary {
    let en_proxy: Vec<f64> = tasks.iter().map(|t| t.en.proxy_in as f64).collect();
    let zh_proxy: Vec<f64> = tasks.iter().map(|t| t.zh.proxy_in as f64).collect();
    let en_rep: Vec<f64> = tasks.iter().map(|t| t.en.cost.input as f64).collect();
    let zh_rep: Vec<f64> = tasks.iter().map(|t| t.zh.cost.input as f64).collect();
    let en_dol: Vec<f64> = tasks.iter().map(|t| t.en.cost.marginal(pricing)).collect();
    let zh_dol: Vec<f64> = tasks.iter().map(|t| t.zh.cost.marginal(pricing)).collect();

    let deltas = |en: &[f64], zh: &[f64]| -> Vec<f64> {
        en.iter().zip(zh).filter_map(|(e, z)| pct_delta(*z, *e)).collect()
    };

    let en_pass = pass_stat(|t| &t.en, tasks);
    let zh_pass = pass_stat(|t| &t.zh, tasks);
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let cost_per_pass = |dollars: &[f64], rate: f64| if rate > 0.0 { mean(dollars) / rate } else { f64::INFINITY };

    let fids: Vec<f64> = tasks.iter().filter_map(|t| t.fidelity.map(|f| f as f64)).collect();

    let (cost_per_pass_ratio, cost_per_pass_ci) = cost_per_pass_ratio_ci(tasks, pricing, 10_000, seed ^ 3);

    EvalSummary {
        n: tasks.len(),
        proxy_input: layer(&en_proxy, &zh_proxy, &deltas(&en_proxy, &zh_proxy), seed),
        reported_input: layer(&en_rep, &zh_rep, &deltas(&en_rep, &zh_rep), seed ^ 1),
        marginal_dollars: layer(&en_dol, &zh_dol, &deltas(&en_dol, &zh_dol), seed ^ 2),
        en_cost_per_pass: cost_per_pass(&en_dol, en_pass.rate),
        zh_cost_per_pass: cost_per_pass(&zh_dol, zh_pass.rate),
        cost_per_pass_ratio,
        cost_per_pass_ci,
        en_pass,
        zh_pass,
        failure_modes_en: modes(|t| &t.en, tasks),
        failure_modes_zh: modes(|t| &t.zh, tasks),
        fidelity_mean: if fids.is_empty() { None } else { Some(mean(&fids)) },
    }
}

fn verdict(ci_lo: f64, ci_hi: f64) -> &'static str {
    if ci_lo <= 0.0 && ci_hi >= 0.0 { "wash (CI crosses 0)" }
    else if ci_hi < 0.0 { "ZH cheaper" }
    else { "EN cheaper" }
}

pub fn build_eval_markdown(run_id: &str, backend: &str, claude_model: &str, s: &EvalSummary) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Sigo coding-eval — `{run_id}`\n");
    let _ = writeln!(o, "- backend: `{backend}`  ·  claude_model: `{claude_model}`  ·  N = {}", s.n);
    let _ = writeln!(o, "- control_mode: `full`  ·  tokenizer: proxy (o200k_base)\n");

    let _ = writeln!(o, "## Headline — three layers (ZH vs EN, paired)\n");
    let _ = writeln!(o, "| Layer | EN | ZH | Δ% | 95% CI | ZH win-rate | Verdict |");
    let _ = writeln!(o, "|---|---:|---:|---:|---:|---:|---|");
    let row = |o: &mut String, name: &str, l: &LayerStat| {
        let _ = writeln!(o, "| {name} | {:.1} | {:.1} | {:+.1}% | [{:+.1}, {:+.1}] | {:.0}% | {} |",
            l.mean_en, l.mean_zh, l.mean_delta_pct, l.ci_lo, l.ci_hi, l.win_rate * 100.0, verdict(l.ci_lo, l.ci_hi));
    };
    row(&mut o, "input tokens (proxy)", &s.proxy_input);
    row(&mut o, "input tokens (reported, uncached)", &s.reported_input);
    row(&mut o, "marginal $ (input+output)", &s.marginal_dollars);

    let _ = writeln!(o, "\n## Correctness\n");
    let _ = writeln!(o, "| Arm | pass | N | pass-rate | Wilson 95% CI | $ / passing task |");
    let _ = writeln!(o, "|---|---:|---:|---:|---:|---:|");
    let cpp = |v: f64| if v.is_finite() { format!("{v:.6}") } else { "∞ (no passes)".to_string() };
    let _ = writeln!(o, "| EN | {} | {} | {:.1}% | [{:.1}, {:.1}]% | {} |",
        s.en_pass.passes, s.en_pass.n, s.en_pass.rate * 100.0, s.en_pass.ci_lo * 100.0, s.en_pass.ci_hi * 100.0, cpp(s.en_cost_per_pass));
    let _ = writeln!(o, "| ZH | {} | {} | {:.1}% | [{:.1}, {:.1}]% | {} |",
        s.zh_pass.passes, s.zh_pass.n, s.zh_pass.rate * 100.0, s.zh_pass.ci_lo * 100.0, s.zh_pass.ci_hi * 100.0, cpp(s.zh_cost_per_pass));
    let ratio_str = if s.cost_per_pass_ratio.is_finite() { format!("{:.2}×", s.cost_per_pass_ratio) } else { "∞".into() };
    let ratio_ci = if s.cost_per_pass_ci.0.is_finite() && s.cost_per_pass_ci.1.is_finite() {
        format!("[{:.2}, {:.2}]", s.cost_per_pass_ci.0, s.cost_per_pass_ci.1)
    } else { "n/a".into() };
    let _ = writeln!(o, "\n- ZH/EN cost-per-passing-task ratio: {ratio_str}  ·  95% CI {ratio_ci}  (>1 means ZH costs more per correct answer)");

    let _ = writeln!(o, "\n## Failure modes\n");
    let fmt_modes = |m: &std::collections::BTreeMap<String, usize>| -> String {
        if m.is_empty() { "none".to_string() }
        else { m.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ") }
    };
    let _ = writeln!(o, "- EN: {}", fmt_modes(&s.failure_modes_en));
    let _ = writeln!(o, "- ZH: {}", fmt_modes(&s.failure_modes_zh));
    if let Some(f) = s.fidelity_mean {
        let _ = writeln!(o, "\n## Round-trip fidelity\n\n- mean EN→ZH→EN closeness: {f:.1} / 10");
    }

    let _ = writeln!(o, "\n## Caveats\n");
    let _ = writeln!(o, "- Proxy token counts are `o200k_base`, NOT Claude's tokenizer; treat as directional.");
    let _ = writeln!(o, "- `claude-code` total input is noisy (cache split is asymmetric across the paired runs); the headline uses **marginal** cost.");
    let _ = writeln!(o, "- N = {}; deltas carry bootstrap CIs but remain point estimates of a small sample.", s.n);
    let _ = writeln!(o, "- Execution runs model-generated code locally; run untrusted corpora in a VM/container.");
    let _ = writeln!(o, "- The local translator is nondeterministic; a re-run may produce slightly different ZH prompts. Models, corpus, and bootstrap seed are fixed within a run.");
    let _ = writeln!(o, "- pass@1 only (`--samples 1`); retries / pass@k are not measured.");
    o
}

pub fn build_eval_csv(tasks: &[TaskEval], pricing: &PricingConfig) -> String {
    let mut o = String::new();
    o.push_str("task_id,category,arm,outcome,proxy_in,reported_in,output,cache_read,cache_write,marginal_dollars,billed_dollars,fidelity\n");
    for t in tasks {
        for (arm, e) in [("en", &t.en), ("zh", &t.zh)] {
            let fid = if arm == "zh" { t.fidelity.map(|v| v.to_string()).unwrap_or_default() } else { String::new() };
            let _ = writeln!(o, "{},{},{},{},{},{},{},{},{},{:.6},{:.6},{}",
                t.task_id, t.category, arm, e.outcome.label(), e.proxy_in,
                e.cost.input, e.cost.output, e.cost.cache_read, e.cost.cache_write,
                e.cost.marginal(pricing), e.cost.billed(pricing), fid);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(outcome: Outcome, input: u32, output: u32, proxy: u32) -> ArmEval {
        ArmEval { outcome, cost: ArmCost { input, output, ..Default::default() }, proxy_in: proxy }
    }

    fn sample() -> Vec<TaskEval> {
        vec![
            TaskEval { task_id: "t1".into(), category: "coding-verifiable".into(),
                en: arm(Outcome::Pass, 100, 200, 90), zh: arm(Outcome::AssertFail, 130, 240, 70), fidelity: Some(8) },
            TaskEval { task_id: "t2".into(), category: "coding-verifiable".into(),
                en: arm(Outcome::Pass, 120, 220, 110), zh: arm(Outcome::Pass, 150, 250, 85), fidelity: Some(6) },
        ]
    }

    #[test]
    fn summarize_computes_pass_rates_and_layers() {
        let s = summarize_eval(&sample(), &PricingConfig::default(), 7);
        assert_eq!(s.n, 2);
        assert_eq!(s.en_pass.passes, 2);
        assert_eq!(s.zh_pass.passes, 1);
        assert!(s.reported_input.mean_delta_pct > 0.0);
        assert!(s.zh_cost_per_pass > s.en_cost_per_pass);
        assert_eq!(s.fidelity_mean, Some(7.0));
        assert!(s.cost_per_pass_ratio > 1.0, "ZH costs more per pass in the sample");
    }

    #[test]
    fn markdown_and_csv_render() {
        let s = summarize_eval(&sample(), &PricingConfig::default(), 7);
        let md = build_eval_markdown("rid", "claude-code", "claude-sonnet-4-6", &s);
        assert!(md.contains("Headline"));
        assert!(md.contains("$ / passing task"));
        assert!(md.contains("cost-per-passing-task ratio"));
        let csv = build_eval_csv(&sample(), &PricingConfig::default());
        assert_eq!(csv.lines().count(), 1 + 2 * 2);
        assert!(csv.lines().next().unwrap().starts_with("task_id,category,arm,outcome"));
    }
}
