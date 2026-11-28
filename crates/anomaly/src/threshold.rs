//! The thresholder: a score becomes a grade and a confidence.
//!
//! Every constant of the grading is here and nowhere else. They are the
//! stock plugin's documented behaviour (a z-score of 3 on a discounted
//! running mean and deviation of past scores) and are checked against the
//! fixtures of `docs/specs/anomaly-detection/SPEC.md` AC-2, never tuned to
//! make a check pass.

/// Standard deviations above the running mean at which the grade leaves zero.
pub(crate) const Z_FACTOR: f64 = 3.0;
/// Further standard deviations over which the grade rises from 0 to 1.
pub(crate) const Z_SPAN: f64 = 2.0;
/// The smallest deviation a score is measured against, so a flat series is not graded on noise.
pub(crate) const MIN_SIGMA: f64 = 0.01;
/// Scores seen before a grade can be above zero.
pub(crate) const WARM_UP: u64 = 32;

/// A discounted running mean and variance of the scores.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Thresholder {
    pub(crate) mean: f64,
    pub(crate) var: f64,
    pub(crate) n: u64,
    /// The discount: the weight of the newest score once the history is long.
    pub(crate) rate: f64,
}

impl Thresholder {
    pub(crate) fn new(sample_size: usize) -> Self {
        Self {
            mean: 0.0,
            var: 0.0,
            n: 0,
            rate: 1.0 / sample_size as f64,
        }
    }

    /// The grade of `score` against what came before it, in [0, 1].
    pub(crate) fn grade(&self, score: f64) -> f64 {
        if self.n < WARM_UP {
            return 0.0;
        }
        let z = (score - self.mean) / self.var.sqrt().max(MIN_SIGMA);
        ((z - Z_FACTOR) / Z_SPAN).clamp(0.0, 1.0)
    }

    /// Folds `score` into the running statistics.
    pub(crate) fn update(&mut self, score: f64) {
        self.n = self.n.saturating_add(1);
        let a = (1.0 / self.n as f64).max(self.rate);
        let delta = score - self.mean;
        self.mean += a * delta;
        self.var = (1.0 - a) * (self.var + a * delta * delta);
    }
}
