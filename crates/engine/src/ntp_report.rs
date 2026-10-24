//! The NTP run as data other tools can read: JSON and CSV with a schema version, and a Nagios-style check.

use crate::ntp::{Consensus, ServerResult, Skipped};
use serde_json::{Value, json};

/// Bumped when a field is renamed, removed or changes meaning. Adding a field does not bump it.
pub const SCHEMA_VERSION: u32 = 1;

pub fn to_json(results: &[ServerResult], consensus: Option<&Consensus>) -> Value {
    json!({ "schema_version": SCHEMA_VERSION, "results": results, "consensus": consensus })
}

const CSV_COLUMNS: &[&str] = &[
    "schema_version",
    "id",
    "host",
    "custom",
    "status",
    "error_kind",
    "stratum",
    "refid",
    "offset_ms",
    "bound_ms",
    "distance_ms",
    "delay_ms",
    "delay_min_ms",
    "delay_p95_ms",
    "jitter_ms",
    "sent",
    "ok",
    "selection",
    "deviation_ms",
    "agrees",
    "kiss",
    "ipv6",
    "nts_ke",
    "nts_authenticated",
    "nts_error_kind",
];

/// One field, quoted when it holds a comma, a quote or a line break, and never starting like a spreadsheet formula.
fn csv_field(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@']) && s.parse::<f64>().is_err() { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

fn num(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.3}")).unwrap_or_default()
}

fn flag(v: Option<bool>) -> String {
    v.map(|b| b.to_string()).unwrap_or_default()
}

/// What happened to a server in one word: `ok`, or the reason nothing was measured.
pub fn status(r: &ServerResult) -> String {
    if r.ok > 0 { "ok".into() } else { r.error_kind.clone().unwrap_or_else(|| "no_answer".into()) }
}

/// One row per server, a header first, lines ended with \n. Empty cells mean "not measured", never zero.
pub fn to_csv(results: &[ServerResult]) -> String {
    let mut out = CSV_COLUMNS.join(",");
    out.push('\n');
    for r in results {
        let sel = r.selection.and_then(|s| serde_json::to_value(s).ok()).and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let cells = [
            SCHEMA_VERSION.to_string(),
            r.id.clone(),
            r.host.clone(),
            r.custom.to_string(),
            status(r),
            r.error_kind.clone().unwrap_or_default(),
            r.stratum.map(|s| s.to_string()).unwrap_or_default(),
            r.refid.clone().unwrap_or_default(),
            num(r.offset_ms),
            num(r.bound_ms),
            num(r.distance_ms),
            num(r.delay_ms),
            num(r.delay_min_ms),
            num(r.delay_p95_ms),
            num(r.jitter_ms),
            r.sent.to_string(),
            r.ok.to_string(),
            sel,
            num(r.deviation_ms),
            flag(r.agrees),
            r.kiss.clone().unwrap_or_default(),
            r.ipv6.clone().unwrap_or_default(),
            r.nts.as_ref().map(|n| n.ok.to_string()).unwrap_or_default(),
            flag(r.nts.as_ref().and_then(|n| n.authenticated)),
            r.nts
                .as_ref()
                .and_then(|n| n.timed.as_ref().and_then(|t| t.error_kind.clone()).or_else(|| n.error_kind.clone()))
                .unwrap_or_default(),
        ];
        out.push_str(&cells.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// What a monitor needs from a check: Nagios' state (0 OK, 1 WARNING, 2 CRITICAL, 3 UNKNOWN) and one line of text with
/// performance data after the `|`.
#[derive(Debug, PartialEq)]
pub struct Check {
    pub state: u8,
    pub line: String,
}

const WORDS: [&str; 4] = ["OK", "WARNING", "CRITICAL", "UNKNOWN"];

/// Nagios ranks a worse state above a better one: CRITICAL, then UNKNOWN, then WARNING, then OK.
fn rank(state: u8) -> u8 {
    match state {
        2 => 3,
        3 => 2,
        s => s,
    }
}

/// How far the server's clock is from ours *beyond what one round trip can hide*: |offset| less its path bound. A
/// reading of 40 ms ± 38 ms may be 2 ms, so it is judged as 2.
pub(crate) fn excess_ms(offset_ms: f64, bound_ms: f64) -> f64 {
    (offset_ms.abs() - bound_ms.max(0.0)).max(0.0)
}

fn label(host: &str) -> String {
    host.chars().filter(|c| !matches!(c, '\'' | '=' | '|' | '\n')).collect()
}

/// A check over the servers measured. The thresholds are on [`excess_ms`], not on the raw offset, so a noisy path doesn't
/// raise an alarm. A server that doesn't answer, or isn't synchronized, is CRITICAL; one that told us to slow down, or
/// that was left alone, is UNKNOWN because nothing was learned about its clock.
pub fn check(results: &[ServerResult], skipped: &[Skipped], warn_ms: f64, crit_ms: f64) -> Check {
    let mut state = 0u8;
    let mut parts: Vec<String> = Vec::new();
    let mut perf: Vec<String> = Vec::new();
    let raise = |s: u8, state: &mut u8| {
        if rank(s) > rank(*state) {
            *state = s;
        }
    };
    for r in results.iter().filter(|r| !r.nts_only) {
        let name = if r.custom { r.host.clone() } else { r.id.clone() };
        let l = label(&name);
        match (r.ok > 0, r.offset_ms, r.bound_ms) {
            (true, Some(off), Some(bound)) => {
                let ex = excess_ms(off, bound);
                let s = if ex > crit_ms {
                    2
                } else if ex > warn_ms {
                    1
                } else {
                    0
                };
                raise(s, &mut state);
                let tag = if s == 0 { String::new() } else { format!(" [{}]", WORDS[s as usize]) };
                parts.push(format!("{name} {off:+.2} ms (±{bound:.2}){tag}"));
                perf.push(format!("'{l}_offset'={off:.3}ms;;;;"));
                perf.push(format!("'{l}_excess'={ex:.3}ms;{warn_ms};{crit_ms};0;"));
                perf.push(format!("'{l}_bound'={bound:.3}ms;;;0;"));
                if let Some(d) = r.delay_ms {
                    perf.push(format!("'{l}_delay'={d:.3}ms;;;0;"));
                }
                if let Some(j) = r.jitter_ms {
                    perf.push(format!("'{l}_jitter'={j:.3}ms;;;0;"));
                }
                if let Some(s) = r.stratum {
                    perf.push(format!("'{l}_stratum'={s};;;0;16"));
                }
            }
            _ => {
                let kind = status(r);
                let s = if kind == "kiss" { 3 } else { 2 };
                raise(s, &mut state);
                parts.push(format!("{name} no usable answer ({kind})"));
            }
        }
    }
    for s in skipped {
        raise(3, &mut state);
        let why = if s.why == "kiss" {
            format!("it asked us to stop ({})", s.kiss.clone().unwrap_or_default())
        } else {
            "asked moments ago".into()
        };
        parts.push(format!("{} left alone: {why}", s.host));
    }
    if parts.is_empty() {
        raise(3, &mut state);
        parts.push("nothing was measured".into());
    }
    let mut line = format!("NTP {} - {}", WORDS[state as usize], parts.join("; "));
    if !perf.is_empty() {
        line.push_str(" | ");
        line.push_str(&perf.join(" "));
    }
    Check { state, line }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(host: &str, offset: f64, bound: f64) -> ServerResult {
        ServerResult {
            id: host.into(),
            host: host.into(),
            ok: 4,
            sent: 4,
            offset_ms: Some(offset),
            bound_ms: Some(bound),
            delay_ms: Some(bound * 2.0),
            stratum: Some(2),
            ..Default::default()
        }
    }

    fn failed(host: &str, kind: &str) -> ServerResult {
        ServerResult {
            id: host.into(),
            host: host.into(),
            sent: 3,
            error_kind: Some(kind.into()),
            error: Some(kind.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_reading_is_judged_beyond_its_own_bound() {
        assert_eq!(excess_ms(40.0, 38.0), 2.0);
        assert_eq!(excess_ms(-40.0, 38.0), 2.0);
        assert_eq!(excess_ms(3.0, 5.0), 0.0);
    }

    #[test]
    fn the_check_states_follow_the_thresholds() {
        let ok = check(&[answered("a", 3.0, 5.0)], &[], 50.0, 200.0);
        assert_eq!(ok.state, 0);
        assert!(ok.line.starts_with("NTP OK - a +3.00 ms (±5.00) | "), "{}", ok.line);
        assert!(ok.line.contains("'a_excess'=0.000ms;50;200;0;"), "{}", ok.line);
        // 90 ms off with 10 ms of path bound is 80 beyond what the path can hide: warning, not critical.
        let warn = check(&[answered("a", -90.0, 10.0)], &[], 50.0, 200.0);
        assert_eq!(warn.state, 1);
        assert!(warn.line.contains("[WARNING]"));
        assert_eq!(check(&[answered("a", 300.0, 10.0)], &[], 50.0, 200.0).state, 2);
        // The same raw offset with a bound as wide is within the noise.
        assert_eq!(check(&[answered("a", 300.0, 290.0)], &[], 50.0, 200.0).state, 0);
    }

    #[test]
    fn failures_are_critical_but_a_kiss_or_a_skip_is_unknown() {
        assert_eq!(check(&[failed("a", "timeout")], &[], 50.0, 200.0).state, 2);
        assert_eq!(check(&[failed("a", "unsynchronized")], &[], 50.0, 200.0).state, 2);
        assert_eq!(check(&[failed("a", "kiss")], &[], 50.0, 200.0).state, 3);
        let skip = Skipped { id: "a".into(), host: "a".into(), why: "recent".into(), until: 0, kiss: None };
        let c = check(&[], std::slice::from_ref(&skip), 50.0, 200.0);
        assert_eq!((c.state, c.line.contains("left alone")), (3, true));
        // The worst of several: critical beats unknown beats warning.
        let mix = check(&[answered("a", 100.0, 1.0), failed("b", "kiss"), failed("c", "dns")], &[], 50.0, 200.0);
        assert_eq!(mix.state, 2);
        let mix = check(&[answered("a", 100.0, 1.0), failed("b", "kiss")], &[], 50.0, 200.0);
        assert_eq!(mix.state, 3);
        assert_eq!(check(&[], &[], 50.0, 200.0).state, 3);
    }

    #[test]
    fn the_csv_has_a_version_a_row_per_server_and_safe_cells() {
        let mut a = answered("time.example", 1.5, 2.0);
        a.refid = Some("GPS,PPS".into());
        let mut b = failed("=cmd()", "dns");
        b.custom = true;
        let csv = to_csv(&[a, b]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("schema_version,id,host,custom,status,error_kind,stratum,refid,offset_ms"));
        assert!(lines[1].starts_with("1,time.example,time.example,false,ok,,2,\"GPS,PPS\",1.500,2.000"), "{}", lines[1]);
        // Not measured is an empty cell, and a name that looks like a formula is defused.
        assert!(lines[2].starts_with("1,'=cmd(),'=cmd(),true,dns,dns,,,,"), "{}", lines[2]);
        assert_eq!(lines[1].split(',').count(), lines[0].split(',').count() + 1); // the quoted comma
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("-5"), "-5");
    }

    #[test]
    fn the_json_carries_the_schema_version() {
        let v = to_json(&[answered("a", 1.0, 1.0)], None);
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["results"][0]["host"], "a");
    }
}
