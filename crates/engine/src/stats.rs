//! Order statistics over round-trip samples.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// Queries sent.
    pub sent: u32,
    /// Answered with the expected outcome (latency statistics use only these).
    pub ok: u32,
    pub timeouts: u32,
    /// DNS errors and unexpected answers (SERVFAIL, REFUSED, rewritten NXDOMAIN…).
    pub errors: u32,
    pub success: f64,
    pub min: f64,
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    pub sd: f64,
    /// Interquartile range, our jitter measure.
    pub iqr: f64,
    /// The mean after pulling the slowest tenth down to the 90th percentile, so one straggler cannot move it;
    /// what the speed score uses.
    pub wmean: f64,
    /// The resolver answered this test normally, then began refusing or went silent, so the figures here are
    /// from the answered part and the test is left out of the score and the answered share.
    pub limited: bool,
    /// Errors that cost in the score (SERVFAIL, REFUSED, other error codes, network errors); a subset of `errors`.
    pub failures: u32,
}

/// Linear-interpolated quantile over sorted data (type 7, as R and NumPy default).
pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let h = (sorted.len() - 1) as f64 * q;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

/// The median of `v` in any order (the mean of the middle pair for an even count), or `None` when there is none.
pub fn median(v: impl Iterator<Item = f64>) -> Option<f64> {
    let mut v: Vec<f64> = v.collect();
    v.sort_by(|a, b| a.total_cmp(b));
    (!v.is_empty()).then(|| quantile(&v, 0.5))
}

/// Nearest-rank value at `q` over sorted data, or `None` when there is none.
pub fn nearest_rank<T: Copy>(sorted: &[T], q: f64) -> Option<T> {
    (!sorted.is_empty()).then(|| sorted[((sorted.len() - 1) as f64 * q).round() as usize])
}

/// `ms` holds round trips (milliseconds) of successful queries only.
pub fn summarize(ms: &mut [f64], sent: u32, timeouts: u32, errors: u32, failures: u32) -> Summary {
    ms.sort_by(|a, b| a.total_cmp(b));
    let n = ms.len();
    let mut s = Summary { sent, ok: n as u32, timeouts, errors, failures, ..Default::default() };
    s.success = if sent == 0 { 0.0 } else { n as f64 / sent as f64 };
    if n == 0 {
        let nan = f64::NAN;
        s.min = nan;
        s.p25 = nan;
        s.p50 = nan;
        s.p75 = nan;
        s.p90 = nan;
        s.p95 = nan;
        s.p99 = nan;
        s.max = nan;
        s.mean = nan;
        s.sd = nan;
        s.iqr = nan;
        s.wmean = nan;
        return s;
    }
    s.min = ms[0];
    s.max = ms[n - 1];
    s.p25 = quantile(ms, 0.25);
    s.p50 = quantile(ms, 0.50);
    s.p75 = quantile(ms, 0.75);
    s.p90 = quantile(ms, 0.90);
    s.p95 = quantile(ms, 0.95);
    s.p99 = quantile(ms, 0.99);
    s.iqr = s.p75 - s.p25;
    s.mean = ms.iter().sum::<f64>() / n as f64;
    s.wmean = ms.iter().map(|x| x.min(s.p90)).sum::<f64>() / n as f64;
    s.sd = if n > 1 { (ms.iter().map(|x| (x - s.mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt() } else { 0.0 };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartiles() {
        let mut v = vec![4.0, 1.0, 3.0, 2.0, 5.0];
        let s = summarize(&mut v, 6, 1, 0, 0);
        assert_eq!(s.p50, 3.0);
        assert_eq!(s.p25, 2.0);
        assert_eq!(s.p75, 4.0);
        assert_eq!(s.ok, 5);
        assert!((s.success - 5.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn winsorised_mean_caps_the_slowest_tenth() {
        let mut v: Vec<f64> = (1..=29).map(f64::from).collect();
        v.push(1000.0);
        let s = summarize(&mut v, 30, 0, 0, 0);
        assert!((s.p90 - 27.1).abs() < 1e-9);
        assert!((s.wmean - (378.0 + 3.0 * 27.1) / 30.0).abs() < 1e-9);
        assert!((s.wmean - 15.31).abs() < 0.01);
        assert!((s.mean - (435.0 + 1000.0) / 30.0).abs() < 1e-9);
        assert!(summarize(&mut [], 0, 0, 0, 0).wmean.is_nan());
    }

    #[test]
    fn median_sorts_and_averages_the_middle_pair() {
        assert_eq!(median([].into_iter()), None);
        assert_eq!(median([7.0].into_iter()), Some(7.0));
        assert_eq!(median([9.0, 1.0, 5.0].into_iter()), Some(5.0));
        assert_eq!(median([8.0, 2.0, 6.0, 4.0].into_iter()), Some(5.0));
    }
}
