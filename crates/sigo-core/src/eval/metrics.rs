use crate::config::PricingConfig;

/// Token usage for one arm of one task.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArmCost {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl ArmCost {
    /// Marginal dollar cost: new input + output only (cached scaffolding excluded).
    pub fn marginal(&self, p: &PricingConfig) -> f64 {
        (self.input as f64 * p.input_per_mtok + self.output as f64 * p.output_per_mtok) / 1e6
    }
    /// Billed dollar cost: marginal + cache read/write at their rates.
    pub fn billed(&self, p: &PricingConfig) -> f64 {
        self.marginal(p)
            + (self.cache_read as f64 * p.cache_read_per_mtok
                + self.cache_write as f64 * p.cache_write_per_mtok)
                / 1e6
    }
}

/// Percentage delta of zh relative to en; `None` if en == 0.
pub fn pct_delta(zh: f64, en: f64) -> Option<f64> {
    if en == 0.0 {
        None
    } else {
        Some((zh - en) / en * 100.0)
    }
}

/// Fraction of paired deltas strictly below zero (ZH cheaper).
pub fn win_rate(deltas: &[f64]) -> f64 {
    if deltas.is_empty() {
        return 0.0;
    }
    deltas.iter().filter(|d| **d < 0.0).count() as f64 / deltas.len() as f64
}

/// Wilson score 95% interval for a binomial proportion.
pub fn wilson_ci(successes: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let z = 1.96f64;
    let phat = successes as f64 / n as f64;
    let nf = n as f64;
    let denom = 1.0 + z * z / nf;
    let centre = phat + z * z / (2.0 * nf);
    let margin = z * ((phat * (1.0 - phat) + z * z / (4.0 * nf)) / nf).sqrt();
    (
        ((centre - margin) / denom).max(0.0),
        ((centre + margin) / denom).min(1.0),
    )
}

/// Minimal seeded PCG32 RNG so bootstrap CIs are reproducible and testable.
pub struct Pcg32 {
    state: u64,
    inc: u64,
}
impl Pcg32 {
    pub fn new(seed: u64) -> Self {
        let mut r = Self {
            state: 0,
            inc: (seed << 1) | 1,
        };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc | 1);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    /// Uniform index in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n.max(1)
    }
}

/// Percentile bootstrap 95% CI for the mean of `samples`.
pub fn bootstrap_ci_mean(samples: &[f64], b: usize, seed: u64) -> (f64, f64) {
    if samples.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let mut rng = Pcg32::new(seed);
    let mut means: Vec<f64> = Vec::with_capacity(b);
    for _ in 0..b {
        let mut acc = 0.0;
        for _ in 0..samples.len() {
            acc += samples[rng.below(samples.len())];
        }
        means.push(acc / samples.len() as f64);
    }
    means.sort_by(|a, c| a.partial_cmp(c).unwrap());
    let lo = means[((0.025 * b as f64) as usize).min(b - 1)];
    let hi = means[((0.975 * b as f64) as usize).min(b - 1)];
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marginal_cost_uses_input_and_output_rates() {
        let p = PricingConfig::default(); // 3 / 15 per Mtok
        let c = ArmCost {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        assert!((c.marginal(&p) - 18.0).abs() < 1e-9);
    }

    #[test]
    fn pct_delta_and_win_rate() {
        assert_eq!(pct_delta(8.0, 10.0), Some(-20.0));
        assert_eq!(pct_delta(1.0, 0.0), None);
        assert!((win_rate(&[-1.0, -2.0, 3.0]) - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn wilson_brackets_known_value() {
        let (lo, hi) = wilson_ci(8, 10);
        assert!(lo > 0.49 && lo < 0.50, "lo={lo}");
        assert!(hi > 0.94 && hi < 0.95, "hi={hi}");
    }

    #[test]
    fn bootstrap_is_deterministic_under_seed() {
        let xs = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let a = bootstrap_ci_mean(&xs, 2000, 42);
        let b = bootstrap_ci_mean(&xs, 2000, 42);
        assert_eq!(a, b);
        assert!(
            a.0 < 4.5 && a.1 > 4.5,
            "CI {a:?} should bracket the mean 4.5"
        );
    }
}
