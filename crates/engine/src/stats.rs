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

/// `ms` holds round trips (milliseconds) of successful queries only.
pub fn summarize(ms: &mut [f64], sent: u32, timeouts: u32, errors: u32) -> Summary {
    ms.sort_by(|a, b| a.total_cmp(b));
    let n = ms.len();
    let mut s = Summary { sent, ok: n as u32, timeouts, errors, ..Default::default() };
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
    s.sd = if n > 1 { (ms.iter().map(|x| (x - s.mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt() } else { 0.0 };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartiles() {
        let mut v = vec![4.0, 1.0, 3.0, 2.0, 5.0];
        let s = summarize(&mut v, 6, 1, 0);
        assert_eq!(s.p50, 3.0);
        assert_eq!(s.p25, 2.0);
        assert_eq!(s.p75, 4.0);
        assert_eq!(s.ok, 5);
        assert!((s.success - 5.0 / 6.0).abs() < 1e-9);
    }
}
