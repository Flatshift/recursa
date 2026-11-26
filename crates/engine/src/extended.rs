//! The Extended test: two to four resolvers asked the same names, one lookup every 75, 300 or 600 ms, 5,000 or 10,000 times.
//!
//! Each slot asks one name of every resolver in a shuffled order, spread across the slot. A lost query never delays
//! the next send: every resolver has enough connections to cover the timeout and a scheduler thread hands each send to a free one. The
//! results are read in bins of 50 slots (50 gaps long); [`analyze`] turns the outcomes so far into the bins, the table,
//! the curves and the call, and is a pure function of them.

use crate::attribution::{GapWatch, Reading, Verdict};
use crate::bench::{self, Deps, Outcome, QUERY_TIMEOUT_MS, RunConfig, RunHandle, Sample, Setup, TargetInfo, TestKind, now_ms};
use crate::catalog::Resolver;
use crate::dns::{self, QuerySpec};
use crate::namelist::{self, Plan};
use crate::net::{self, Exchanger, Transport};
use crate::stats::round2;
use crate::{clock, link, power, stats};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

/// Slots in a bin. A bin takes `PER_BIN` gaps: 3.75, 15 or 30 seconds.
const PER_BIN: usize = 50;
/// Slots between live chart updates, the last bin partly filled.
const EMIT_EVERY: usize = 10;
/// Bins in one cold bin.
const COLD_BINS: usize = 5;
/// A lost query ranks as this slow.
const LOST_MS: f64 = QUERY_TIMEOUT_MS as f64;
/// The kept bins are cut into this many stretches to size the range of a gap.
const BATCHES: usize = 10;
/// Fewer kept slots than this say too little to give a range or a call.
const MIN_SLOTS: usize = 500;
/// Fewer kept once-asked slots than this say too little for a cold ranking's range or call.
const MIN_COLD_SLOTS: usize = 100;
/// Two-sided 95% Student t for nine degrees of freedom, Bonferroni-adjusted for 1, 2 and 3 comparisons.
const T9: [f64; 3] = [2.262, 2.685, 2.933];
/// Sent before the measured phase on every connection: a name no run is about.
const WARM_NAME: &str = "example.net";
/// A failure this soon before a sleep, with no answer, is taken as the sleep's.
const EXCUSE_BEFORE_NS: u64 = 2_000_000_000;
/// Thresholds of the cumulative curve: 1/16 decade steps from 1 ms, then the timeout.
const ECDF_STEPS: u32 = 52;
/// The result's groups, in the order shown.
const GROUPS: [&str; 6] = ["common_first", "common_later", "tail", "tail_com", "tail_generic", "tail_cc"];

// ---- What the run's state carries.

/// Where the names came from.
#[derive(Clone, Debug, Serialize)]
pub struct NamesUsed {
    /// Always `bundled`.
    pub source: &'static str,
    pub list_id: String,
    pub list_date: String,
    pub seed: u64,
    /// Common names asked (each eight times), and tail names.
    pub common: u32,
    pub tail: u32,
    /// The registry (`arin`, `ripe`, …) that serves the user's country, when the country was known, and how many of
    /// the tail names came from its countries.
    pub region: Option<&'static str>,
    pub regional: u32,
}

/// One resolver's lookups in one bin: those counted, and the times of those answered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BinCell {
    pub sent: u32,
    pub lost: u32,
    pub p5: Option<f64>,
    pub p25: Option<f64>,
    pub p50: Option<f64>,
    pub p75: Option<f64>,
    pub p95: Option<f64>,
}

/// A bin of 50 slots. `at` and `end` are ms since the run started, on a clock that keeps running through sleep.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bin {
    pub at: i64,
    pub end: i64,
    /// Every resolver lost half or more of its queries, so the bin is in no figure.
    pub removed: bool,
    /// By target index.
    pub r: Vec<BinCell>,
    /// By target index, the later asks of common names alone. Empty on a cold bin.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rep: Vec<BinCell>,
    /// By target index, the counted lookups: repeat asks and names asked once, leaving out first asks. What the headline ranks. Empty on a cold bin.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<BinCell>,
    /// By target index, the first asks of common names alone, which fill the caches. Empty on a cold bin.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub first: Vec<BinCell>,
}

/// The gap to the lead, with its range.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Vs {
    pub diff: f64,
    pub lo: f64,
    pub hi: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    /// Target index.
    pub t: usize,
    pub sent: u32,
    pub lost: u32,
    pub loss_pct: f64,
    /// Queries left out because this resolver answered the name with a block or an error another one didn't.
    pub blocked: u32,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p95: Option<f64>,
    pub vs: Option<Vs>,
    /// `lead`, `tie`, `slower` or `unclear`; none until the run has ended with enough to say.
    pub call: Option<&'static str>,
}

/// Share of counted queries answered within each threshold, by target index.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Ecdf {
    pub x: Vec<f64>,
    pub r: Vec<Vec<f64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupCell {
    pub p50: Option<f64>,
    pub lost: u32,
}

/// Names of one kind: how many slots, and each resolver's median and losses on them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Group {
    pub key: &'static str,
    pub count: u32,
    pub r: Vec<GroupCell>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ExtResult {
    /// Every slot ran.
    #[serde(rename = "final")]
    pub is_final: bool,
    pub no_verdict: bool,
    pub margin: f64,
    pub loss_flag: bool,
    /// Seconds of removed bins.
    pub removed_s: f64,
    pub dropped_names: u32,
    /// Best first.
    pub rows: Vec<Row>,
    /// The ranking over the once-asked (tail) names alone, best first; `blocked` is 0.
    pub cold: Vec<Row>,
    pub cold_margin: f64,
    pub ecdf: Ecdf,
    pub groups: Vec<Group>,
}

/// `[start_ms, end_ms, cause]`: a stretch with nothing sent, in ms since the run started.
pub type GapSpan = (i64, i64, &'static str);

#[derive(Clone, Debug, Serialize)]
pub struct ExtendedState {
    /// Slots, which is lookups per resolver.
    pub total: u32,
    pub per_bin: u32,
    pub bin_s: f64,
    /// Seconds in a cold bin. Fractions of a second at the Fast pace; runs before that stored whole seconds.
    pub cold_bin_s: f64,
    pub gap_ms: u32,
    pub timeout_ms: u32,
    pub names: NamesUsed,
    pub plan: Plan,
    /// Per slot, the targets in the order they were asked.
    pub order: Vec<Vec<u8>>,
    pub bins: Vec<Bin>,
    /// The once-asked lookups in wider bins, each `COLD_BINS` bins of `bins` long.
    pub cold_bins: Vec<Bin>,
    pub gaps: Vec<GapSpan>,
    pub result: ExtResult,
}

impl ExtendedState {
    /// The call in a line, as the run list and the terminal show it.
    pub fn verdict(&self, targets: &[TargetInfo]) -> String {
        let r = &self.result;
        let name = |t: usize| format!("{} {}", targets[t].provider, targets[t].variant).trim().to_string();
        let (Some(lead), Some(next)) = (r.rows.first(), r.rows.get(1)) else { return "No verdict".into() };
        if !r.is_final {
            return "Stopped early, no verdict".into();
        }
        if r.no_verdict {
            return "No verdict: too much of the run was lost to the link".into();
        }
        match (next.call, &next.vs) {
            (Some("slower"), Some(v)) => format!("{} has the lowest median, {:.1} ms ahead of {}", name(lead.t), v.diff, name(next.t)),
            (Some("tie"), _) => format!("Practical tie for first: {} or {}", name(lead.t), name(next.t)),
            (Some("unclear"), _) => format!("Too close to call: {} or {}", name(lead.t), name(next.t)),
            _ => "No verdict".into(),
        }
    }
}

/// The run list's summary of an Extended run.
pub(crate) fn summary(s: &bench::RunState, e: &ExtendedState) -> serde_json::Value {
    let label = |t: usize| format!("{} {}", s.targets[t].provider, s.targets[t].variant).trim().to_string();
    let r = &e.result;
    let best = r.rows.first().filter(|_| r.is_final && !r.no_verdict).map(|lead| {
        let tied = r.rows.get(1).filter(|n| n.call == Some("tie")).map(|n| label(n.t));
        json!({
            "resolver_id": s.targets[lead.t].resolver_id, "label": label(lead.t),
            "transport": s.targets[lead.t].transport, "composite_ms": lead.p50,
            "cdn_penalty_ms": null, "success": 1.0 - lead.loss_pct / 100.0, "tied_with": tied,
        })
    });
    json!({
        "mode": "extended",
        "verdict": e.verdict(&s.targets),
        "final": r.is_final,
        "best": best,
        "resolvers": s.resolvers.len(),
        "targets": s.targets.len(),
        "queries": s.samples.len(),
        "tests": [],
        "transports": s.config.transports,
        "vantage": s.env.pointer("/vantage/network/holder"),
    })
}

// ---- Analysis.

/// What [`analyze`] reads: the outcome of every slot for every target (none while not yet in), the plan and the order.
pub(crate) struct Input<'a> {
    /// Seconds in a bin.
    pub bin_s: f64,
    /// By slot, then target index.
    pub grid: &'a [Vec<Option<Sample>>],
    pub plan: &'a Plan,
    pub order: &'a [Vec<u8>],
    /// Targets that were measured.
    pub live: &'a [bool],
    /// Slots in a full run.
    pub total: usize,
}

pub(crate) struct Analysis {
    pub bins: Vec<Bin>,
    pub cold_bins: Vec<Bin>,
    pub result: ExtResult,
    /// Targets that answered tail names faster when another resolver had just been asked them.
    pub shared_cache: Vec<usize>,
}

/// A query that counts: from target `t`, in slot `q`, with its time when answered.
struct Counted {
    t: usize,
    q: usize,
    bin: usize,
    ms: Option<f64>,
}

#[derive(Default)]
struct Seen {
    ok: bool,
    /// Targets that answered with a negative or filtered outcome.
    bad: u32,
}

fn lost(o: Outcome) -> bool {
    o == Outcome::Timeout || o.costs()
}

fn pct(sorted: &[f64], p: f64) -> Option<f64> {
    (!sorted.is_empty()).then(|| round2(stats::quantile(sorted, p)))
}

/// The times of `v` in order, a lost query at the timeout.
fn ranked<'a>(v: impl Iterator<Item = &'a Option<f64>>) -> Vec<f64> {
    let mut out: Vec<f64> = v.map(|m| m.unwrap_or(LOST_MS)).collect();
    out.sort_by(|a, b| a.total_cmp(b));
    out
}

fn median_of(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| stats::round1(stats::quantile(v, 0.5)))
}

/// Which of [`GROUPS`] slot `q` belongs to: its own kind, and for a tail name its TLD group as well.
fn kinds(plan: &Plan, q: usize) -> [&'static str; 2] {
    match plan.touch[q] {
        0 => ["tail", ["tail_com", "tail_generic", "tail_cc"][namelist::class(&plan.names[q])]],
        1 => ["common_first", ""],
        _ => ["common_later", ""],
    }
}

/// The ranking of the live targets over `by` (each target's counted times by bin) in the kept bins, best first, and the
/// margin below which a gap counts as a tie. With `enough`, each row behind the lead gets its gap with a range, and with
/// `call` the rows get their calls as well.
fn rank(live: &[usize], by: &[Vec<Vec<Option<f64>>>], kept: &[usize], blocked: &[u32], enough: bool, call: bool) -> (Vec<Row>, f64) {
    let vals = |t: usize, bs: &[usize]| ranked(bs.iter().flat_map(|&b| by[t][b].iter()));
    let mut rows: Vec<Row> = live
        .iter()
        .map(|&t| {
            let all = vals(t, kept);
            let sent = all.len() as u32;
            let gone = kept.iter().map(|&b| by[t][b].iter().filter(|m| m.is_none()).count() as u32).sum::<u32>();
            let q = |p: f64| (!all.is_empty()).then(|| stats::round1(stats::quantile(&all, p)));
            Row {
                t,
                sent,
                lost: gone,
                loss_pct: if sent == 0 { 0.0 } else { round2(gone as f64 / sent as f64 * 100.0) },
                blocked: blocked[t],
                p50: q(0.5),
                p90: q(0.9),
                p95: q(0.95),
                vs: None,
                call: None,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.p50.unwrap_or(f64::MAX).total_cmp(&b.p50.unwrap_or(f64::MAX)).then(a.lost.cmp(&b.lost)).then(a.t.cmp(&b.t)));
    let margin = rows.first().and_then(|r| r.p50).map_or(3.0, |p| f64::max(3.0, stats::round1(0.05 * p)));

    // The gap to the lead: the kept bins cut into ten stretches of consecutive bins, the gap of medians taken again with each
    // stretch left out in turn, and a t-interval from the spread of those ten around the gap of the whole.
    let cut = |k: usize| k * kept.len() / BATCHES;
    let comparisons = rows.len().saturating_sub(1).clamp(1, 3);
    let gap = |a: usize, b: usize| -> Option<(f64, f64, f64)> {
        if kept.len() < BATCHES {
            return None;
        }
        let diff = stats::round1(median_of(&vals(b, kept))? - median_of(&vals(a, kept))?);
        let g: Option<Vec<f64>> = (0..BATCHES)
            .map(|k| {
                let bs = [&kept[..cut(k)], &kept[cut(k + 1)..]].concat();
                // Unrounded: medians rounded to a tenth can leave all ten gaps equal, and the range with no width.
                let mid = |t: usize| Some(vals(t, &bs)).filter(|v| !v.is_empty()).map(|v| stats::quantile(&v, 0.5));
                Some(mid(b)? - mid(a)?)
            })
            .collect();
        let g = g?;
        let mean = g.iter().sum::<f64>() / BATCHES as f64;
        let se = (g.iter().map(|x| (x - mean).powi(2)).sum::<f64>() * (BATCHES - 1) as f64 / BATCHES as f64).sqrt();
        let half = T9[comparisons - 1] * se;
        Some((diff, diff - half, diff + half))
    };
    if let Some(lead) = rows.first().map(|r| r.t).filter(|_| enough) {
        rows[0].call = call.then_some("lead");
        for r in rows.iter_mut().skip(1) {
            let Some((diff, lo, hi)) = gap(lead, r.t) else { continue };
            r.vs = Some(Vs { diff, lo: round2(lo), hi: round2(hi) });
            r.call = call.then(|| {
                if lo > margin {
                    "slower"
                } else if hi < margin && lo > -margin {
                    "tie"
                } else {
                    "unclear"
                }
            });
        }
    }
    (rows, margin)
}

pub(crate) fn analyze(inp: &Input) -> Analysis {
    let n = inp.live.len();
    let live: Vec<usize> = (0..n).filter(|&t| inp.live[t]).collect();
    // Only slots every measured target has an outcome for.
    let done = inp.grid.iter().take_while(|row| live.iter().all(|&t| row[t].is_some())).count();
    let nbins = done.div_ceil(PER_BIN);
    let slots = |b: usize| b * PER_BIN..((b + 1) * PER_BIN).min(done);
    // An excused outcome was not sent, and is not lost.
    let cell = |q: usize, t: usize| inp.grid[q][t].filter(|s| !s.x);

    // A bin where every target lost half or more of what it sent is the link's, not a resolver's.
    let removed: Vec<bool> = (0..nbins)
        .map(|b| {
            !live.is_empty()
                && live.iter().all(|&t| {
                    let (sent, gone) = slots(b).filter_map(|q| cell(q, t)).fold((0, 0), |(s, l), x| (s + 1, l + lost(x.o) as usize));
                    sent > 0 && gone * 2 >= sent
                })
        })
        .collect();
    let removed_n = removed.iter().filter(|r| **r).count();

    // In the kept bins: a name nobody answered is dropped for all, and a name one resolver answered with a block or an
    // error that another didn't is out of that resolver's timing. This is kept by name, not by ask: a common name is
    // asked several times, and one such answer takes all of them out for that resolver.
    let mut seen: HashMap<&str, Seen> = HashMap::new();
    for q in (0..done).filter(|q| !removed[q / PER_BIN]) {
        for &t in &live {
            let Some(s) = cell(q, t) else { continue };
            let e = seen.entry(inp.plan.names[q].as_str()).or_default();
            match s.o {
                Outcome::Ok => e.ok = true,
                Outcome::Nxdomain | Outcome::Nodata | Outcome::Blocked | Outcome::Hijack => e.bad |= 1 << t,
                _ => {}
            }
        }
    }
    let dropped_names = seen.values().filter(|e| !e.ok).count() as u32;
    let dropped = |q: usize| seen.get(inp.plan.names[q].as_str()).is_some_and(|e| !e.ok);

    let mut counted: Vec<Counted> = Vec::new();
    let mut blocked = vec![0u32; n];
    for q in (0..done).filter(|&q| !dropped(q)) {
        let (bin, bad) = (q / PER_BIN, seen.get(inp.plan.names[q].as_str()).map_or(0, |e| e.bad));
        for &t in &live {
            let Some(s) = cell(q, t) else { continue };
            if bad >> t & 1 == 1 {
                blocked[t] += !removed[bin] as u32;
            } else if s.o == Outcome::Ok || lost(s.o) {
                counted.push(Counted { t, q, bin, ms: (s.o == Outcome::Ok).then(|| s.us as f64 / 1000.0) });
            }
        }
    }
    // The counted queries by target, then bin.
    let mut by: Vec<Vec<Vec<Option<f64>>>> = vec![vec![Vec::new(); nbins]; n];
    for c in &counted {
        by[c.t][c.bin].push(c.ms);
    }

    // The counted queries of the repeat asks (`touch >= 2`), of the first asks (`touch == 1`) and of all but the first
    // asks, by target, then bin.
    let mut by_rep: Vec<Vec<Vec<Option<f64>>>> = vec![vec![Vec::new(); nbins]; n];
    let mut by_first: Vec<Vec<Vec<Option<f64>>>> = vec![vec![Vec::new(); nbins]; n];
    let mut head: Vec<Vec<Vec<Option<f64>>>> = vec![vec![Vec::new(); nbins]; n];
    for c in &counted {
        let touch = inp.plan.touch[c.q];
        if touch >= 2 {
            by_rep[c.t][c.bin].push(c.ms);
        }
        if touch == 1 {
            by_first[c.t][c.bin].push(c.ms);
        }
        if touch != 1 {
            head[c.t][c.bin].push(c.ms);
        }
    }
    // The queries of the names asked once, by target, then bin.
    let mut cold_by: Vec<Vec<Vec<Option<f64>>>> = vec![vec![Vec::new(); nbins]; n];
    for c in counted.iter().filter(|c| inp.plan.touch[c.q] == 0) {
        cold_by[c.t][c.bin].push(c.ms);
    }

    let bin_cell = |v: &[Option<f64>]| {
        let mut ok: Vec<f64> = v.iter().flatten().copied().collect();
        ok.sort_by(|a, b| a.total_cmp(b));
        let sent = v.len() as u32;
        BinCell {
            sent,
            lost: sent - ok.len() as u32,
            p5: pct(&ok, 0.05),
            p25: pct(&ok, 0.25),
            p50: pct(&ok, 0.5),
            p75: pct(&ok, 0.75),
            p95: pct(&ok, 0.95),
        }
    };
    let bins: Vec<Bin> = (0..nbins)
        .map(|b| {
            let at = slots(b).flat_map(|q| live.iter().filter_map(move |&t| inp.grid[q][t])).map(|s| s.at as i64);
            let (lo, hi) = at.fold((i64::MAX, 0), |(lo, hi), a| (lo.min(a), hi.max(a)));
            let r = (0..n).map(|t| bin_cell(&by[t][b])).collect();
            let rep = (0..n).map(|t| bin_cell(&by_rep[t][b])).collect();
            let all = (0..n).map(|t| bin_cell(&head[t][b])).collect();
            let first = (0..n).map(|t| bin_cell(&by_first[t][b])).collect();
            Bin { at: lo.min(hi), end: hi, removed: removed[b], r, rep, all, first }
        })
        .collect();
    // One cold bin for each run of `COLD_BINS` fine bins, from the once-asked lookups in those that are kept.
    let cold_bins: Vec<Bin> = (0..nbins.div_ceil(COLD_BINS))
        .map(|g| {
            let members = g * COLD_BINS..((g + 1) * COLD_BINS).min(nbins);
            let kept_in: Vec<usize> = members.clone().filter(|&b| !removed[b]).collect();
            let r = (0..n).map(|t| bin_cell(&kept_in.iter().flat_map(|&b| cold_by[t][b].iter().copied()).collect::<Vec<_>>())).collect();
            Bin {
                at: bins[members.start].at,
                end: bins[members.end - 1].end,
                removed: kept_in.is_empty(),
                r,
                rep: Vec::new(),
                all: Vec::new(),
                first: Vec::new(),
            }
        })
        .collect();

    let kept: Vec<usize> = (0..nbins).filter(|&b| !removed[b]).collect();
    let kept_slots: usize = kept.iter().map(|&b| slots(b).len()).sum();
    let is_final = done > 0 && done == inp.total;
    let no_verdict = removed_n as f64 > 0.2 * nbins as f64;
    let call = is_final && !no_verdict;
    // The first ask of each common name only fills the caches, so the ranking leaves it out.
    let (rows, margin) = rank(&live, &head, &kept, &blocked, kept_slots >= MIN_SLOTS, call);

    // The same ranking over the names asked once.
    let cold_slots = kept.iter().flat_map(|&b| slots(b)).filter(|&q| inp.plan.touch[q] == 0 && !dropped(q)).count();
    let (cold, cold_margin) = rank(&live, &cold_by, &kept, &vec![0; n], cold_slots >= MIN_COLD_SLOTS, call);
    let loss_flag = matches!((rows.first(), rows.get(1)), (Some(a), Some(b)) if a.loss_pct - b.loss_pct >= 1.0);

    // The share answered within each threshold.
    let mut x: Vec<f64> = (0..=ECDF_STEPS).map(|k| round2(10f64.powf(k as f64 / 16.0))).collect();
    x.push(LOST_MS);
    let r: Vec<Vec<f64>> = (0..n)
        .map(|t| {
            let mut ok: Vec<f64> = kept.iter().flat_map(|&b| head[t][b].iter().flatten().copied()).collect();
            ok.sort_by(|a, b| a.total_cmp(b));
            let sent = kept.iter().map(|&b| head[t][b].len()).sum::<usize>().max(1) as f64;
            x.iter().map(|&v| (ok.partition_point(|&m| m <= v) as f64 / sent * 10_000.0).round() / 10_000.0).collect()
        })
        .collect();
    let ecdf = Ecdf { x, r };

    let kept_counted = || counted.iter().filter(|c| !removed[c.bin]);
    let groups: Vec<Group> = GROUPS
        .iter()
        .map(|&key| {
            let count = (0..done).filter(|&q| !removed[q / PER_BIN] && !dropped(q) && kinds(inp.plan, q).contains(&key)).count();
            let r = (0..n)
                .map(|t| {
                    let mine: Vec<Option<f64>> =
                        kept_counted().filter(|c| c.t == t && kinds(inp.plan, c.q).contains(&key)).map(|c| c.ms).collect();
                    GroupCell { p50: median_of(&ranked(mine.iter())), lost: mine.iter().filter(|m| m.is_none()).count() as u32 }
                })
                .collect();
            Group { key, count: count as u32, r }
        })
        .collect();

    // A tail name answered faster by a resolver asked after another suggests the name was cached by then.
    let mut shared_cache = Vec::new();
    if kept_slots >= MIN_SLOTS {
        for &t in &live {
            let tail = || kept_counted().filter(|c| c.t == t && inp.plan.touch[c.q] == 0);
            let first = |c: &&Counted| inp.order[c.q].first() == Some(&(t as u8));
            let asked_first: Vec<Option<f64>> = tail().filter(first).map(|c| c.ms).collect();
            let asked_later: Vec<Option<f64>> = tail().filter(|c| !first(c)).map(|c| c.ms).collect();
            let (Some(f), Some(l)) = (median_of(&ranked(asked_first.iter())), median_of(&ranked(asked_later.iter()))) else { continue };
            if f - l > margin && f - l > 0.25 * l {
                shared_cache.push(t);
            }
        }
    }

    let result = ExtResult {
        is_final,
        no_verdict,
        margin,
        loss_flag,
        removed_s: removed_n as f64 * inp.bin_s,
        dropped_names,
        rows,
        cold,
        cold_margin,
        ecdf,
        groups,
    };
    Analysis { bins, cold_bins, result, shared_cache }
}

// ---- The schedule.

struct Job {
    q: usize,
}

/// Word from the scheduler to whoever collects.
pub(crate) enum Msg {
    Done { sample: Sample, sent_ns: u64 },
    Gap { span: GapSpan, since_ns: u64, until_ns: u64 },
}

pub(crate) enum Ev {
    Msg(Msg),
    /// Nothing arrived for a while (or something did): flush what is waiting.
    Tick,
}

/// What the scheduler is told before each send.
pub(crate) enum Pace {
    Go,
    /// The computer slept or stalled: nothing is sent until it settles.
    Hold,
    /// It has settled; the stretch began at `since_ns` and ends at `until_ns` (on the clock that keeps running in sleep).
    Resumed {
        since_ns: u64,
        until_ns: u64,
        cause: &'static str,
    },
}

/// What a schedule needs.
pub(crate) struct Drive<'a> {
    pub slot: Duration,
    pub timeout: Duration,
    pub plan: &'a Plan,
    pub order: &'a [Vec<u8>],
    pub cancel: &'a AtomicBool,
    /// The reading of the clock that runs through sleep (`clock::awake_total_ns().1`) when the run started.
    pub origin_ns: u64,
}

/// Runs the schedule to the end of the plan or a cancel, with `conns` as each target's connections (none: not
/// measured), calling `on` for every message and about every 100 ms. Returns when every connection has finished.
pub(crate) fn drive(d: &Drive, conns: Vec<Vec<Box<dyn Exchanger>>>, pace: impl FnMut() -> Pace + Send, on: &mut dyn FnMut(Ev)) {
    let (mtx, mrx) = mpsc::channel::<Msg>();
    std::thread::scope(|sc| {
        let mut jobs: Vec<Option<mpsc::Sender<Job>>> = Vec::new();
        for (t, workers) in conns.into_iter().enumerate() {
            if workers.is_empty() {
                jobs.push(None);
                continue;
            }
            let (jtx, jrx) = mpsc::channel::<Job>();
            let jrx = std::sync::Arc::new(Mutex::new(jrx));
            for mut x in workers {
                let (jrx, mtx) = (jrx.clone(), mtx.clone());
                sc.spawn(move || worker(d, t, x.as_mut(), &jrx, &mtx));
            }
            jobs.push(Some(jtx));
        }
        sc.spawn(move || schedule(d, jobs, pace, mtx));
        loop {
            match mrx.recv_timeout(Duration::from_millis(100)) {
                Ok(m) => on(Ev::Msg(m)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            on(Ev::Tick);
        }
    });
}

/// Takes the next send for target `t`, whichever connection is free, and reports its outcome.
fn worker(d: &Drive, t: usize, x: &mut dyn Exchanger, jobs: &Mutex<mpsc::Receiver<Job>>, out: &mpsc::Sender<Msg>) {
    clock::prioritize_thread();
    loop {
        let job = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok(Job { q }) = job else { break };
        let kind = if d.plan.touch[q] > 0 { TestKind::Cached } else { TestKind::Uncached };
        // Encoding happens before the send stamp, outside the measured interval.
        let wire = dns::encode_query(dns::new_id(), &QuerySpec::new(d.plan.names[q].clone(), dns::TYPE_A)).ok();
        let sent_ns = clock::awake_total_ns().1;
        let at = (sent_ns.saturating_sub(d.origin_ns) / 1_000_000).min(u32::MAX as u64) as u32;
        let (o, us, ttl, ip_ttl) = match wire {
            Some(w) => {
                let res = x.exchange(&w, d.timeout);
                let (o, us, ttl) = bench::classify(TestKind::Cached, &res);
                (o, us, ttl, res.as_ref().ok().and_then(|r| r.ttl).unwrap_or(0))
            }
            None => (Outcome::Neterr, 0, None, 0),
        };
        let sample = Sample { t: t as u16, k: kind, i: q as u32, us, o, at, h: ip_ttl, d: ttl, x: false };
        let _ = out.send(Msg::Done { sample, sent_ns });
    }
}

/// Slot `q` starts at `q` slots of awake time; its sends are spread across the slot in the order recorded.
fn schedule(d: &Drive, jobs: Vec<Option<mpsc::Sender<Job>>>, mut pace: impl FnMut() -> Pace, out: mpsc::Sender<Msg>) {
    clock::prioritize_thread();
    let awake = || Duration::from_nanos(clock::awake_total_ns().0);
    let mut base = awake();
    for (q, order) in d.order.iter().enumerate() {
        for (p, &t) in order.iter().enumerate() {
            let due = d.slot * q as u32 + d.slot * p as u32 / order.len() as u32;
            loop {
                if d.cancel.load(Ordering::Relaxed) {
                    return;
                }
                match pace() {
                    Pace::Hold => {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Pace::Resumed { since_ns, until_ns, cause } => {
                        // Carry on from the next send, with no burst to catch up.
                        base = awake().saturating_sub(due);
                        let end = (clock::awake_total_ns().1.saturating_sub(d.origin_ns) / 1_000_000) as i64;
                        let start = end - (until_ns.saturating_sub(since_ns) / 1_000_000) as i64;
                        let _ = out.send(Msg::Gap { span: (start, end, cause), since_ns, until_ns });
                    }
                    Pace::Go => {}
                }
                let now = awake();
                match (base + due).checked_sub(now).filter(|r| !r.is_zero()) {
                    Some(left) => std::thread::sleep(left.min(Duration::from_millis(50))),
                    None => {
                        // Held up for a moment too short to be a gap: carry on from here, with no burst to catch up.
                        if now.saturating_sub(base + due) > (d.slot * 4).min(Duration::from_secs(1)) {
                            base = now.saturating_sub(due);
                        }
                        break;
                    }
                }
            }
            if let Some(tx) = &jobs[t as usize] {
                let _ = tx.send(Job { q });
            }
        }
    }
}

fn reading() -> Reading {
    let (awake, total) = clock::awake_total_ns();
    let p = power::watch();
    Reading { awake, total, slept_at: p.slept_at.load(Ordering::Relaxed), woke_at: p.woke_at.load(Ordering::Relaxed) }
}

/// The watch for sleeps and stalls, as the monitor keeps it, as a [`Pace`] for the scheduler, which asks at least
/// every 50 ms.
fn watch() -> impl FnMut() -> Pace + Send {
    let mut w = GapWatch::ticking_every(50_000_000);
    move || {
        let r = reading();
        if !w.tick(r) {
            return Pace::Hold;
        }
        match w.in_gap().then(|| w.end_cycle(r, true)) {
            Some(Verdict::Keep(Some(g))) => Pace::Resumed { since_ns: g.since, until_ns: r.total, cause: g.cause.as_str() },
            _ => Pace::Go,
        }
    }
}

// ---- The run.

/// Whether the Extended test can start: two to four resolvers, or one with `aa`.
pub(crate) fn check(cfg: &RunConfig, all: &[Resolver]) -> Result<(), String> {
    let t = cfg.transports.first().copied().unwrap_or(Transport::Udp);
    let v6 = cfg.family == bench::Family::V6;
    let n = cfg
        .resolvers
        .iter()
        .filter_map(|id| all.iter().find(|r| &r.id == id))
        .filter(|r| (bench::offers(r, t) || !r.udp && (r.doh.is_some() || r.dot.is_some())) && r.primary(v6).is_some())
        .count();
    if (2..=4).contains(&n) || n == 1 && cfg.aa { Ok(()) } else { Err("The Extended test compares two to four resolvers.".into()) }
}

/// Collects what the schedule reports: samples to the UI, a bin at a time to the analysis.
struct Collector<'a> {
    h: &'a RunHandle,
    bin_s: f64,
    plan: &'a Plan,
    order: &'a [Vec<u8>],
    live: &'a [bool],
    total: usize,
    grid: Vec<Vec<Option<Sample>>>,
    /// When each sample was sent (the clock that runs in sleep), in the order they were recorded.
    sent_ns: Vec<u64>,
    batch: Vec<Sample>,
    last_flush: Instant,
    gaps: Vec<GapSpan>,
    /// Sends in these stretches that got no answer are the sleep's.
    windows: Vec<(u64, u64)>,
    /// Slots every measured target has an outcome for, from the first.
    done: usize,
    /// Slots already sent to the UI.
    emitted: usize,
}

impl Collector<'_> {
    fn on(&mut self, ev: Ev) {
        match ev {
            Ev::Msg(Msg::Done { mut sample, sent_ns }) => {
                let hit = |&(lo, hi): &(u64, u64)| sent_ns >= lo && sent_ns < hi;
                sample.x = sample.o != Outcome::Ok && self.windows.iter().any(hit);
                self.grid[sample.i as usize][sample.t as usize] = Some(sample);
                self.sent_ns.push(sent_ns);
                self.batch.push(sample);
                while self.done < self.total && self.row_complete(self.done) {
                    self.done += 1;
                }
                let upto = (self.done / EMIT_EVERY) * EMIT_EVERY;
                if upto > self.emitted {
                    self.emit(upto);
                }
            }
            Ev::Msg(Msg::Gap { span, since_ns, until_ns }) => {
                self.gaps.push(span);
                let window = (since_ns.saturating_sub(EXCUSE_BEFORE_NS), until_ns);
                self.windows.push(window);
                self.excuse(window);
            }
            Ev::Tick => {}
        }
        if self.batch.len() >= 256 || self.last_flush.elapsed() >= Duration::from_millis(125) {
            self.flush();
        }
    }

    fn row_complete(&self, q: usize) -> bool {
        self.grid[q].iter().zip(self.live).all(|(s, live)| s.is_some() || !live)
    }

    fn flush(&mut self) {
        self.last_flush = Instant::now();
        if self.batch.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.batch);
        self.h.publish(|s| {
            s.samples.extend_from_slice(&batch);
            s.done += batch.len() as u32;
            json!({"type": "samples", "samples": batch, "done": s.done, "total": s.total})
        });
    }

    /// Marks what was sent in `window` and never answered, though it was recorded before the sleep was known.
    fn excuse(&mut self, (lo, hi): (u64, u64)) {
        self.flush();
        let hits: Vec<usize> = (0..self.sent_ns.len()).filter(|&i| self.sent_ns[i] >= lo && self.sent_ns[i] < hi).collect();
        let grid = &mut self.grid;
        self.h.with(|s| {
            for i in hits {
                let x = &mut s.samples[i];
                if x.o != Outcome::Ok && !x.x {
                    x.x = true;
                    if let Some(c) = grid[x.i as usize][x.t as usize].as_mut() {
                        c.x = true;
                    }
                }
            }
        });
    }

    /// Reads the first `upto` slots and sends the bins, gaps and result. Every bin goes in the event: a late answer or
    /// a sleep can change one already sent.
    fn emit(&mut self, upto: usize) -> Analysis {
        self.flush();
        let a = analyze(&Input {
            bin_s: self.bin_s,
            grid: &self.grid[..upto.min(self.total)],
            plan: self.plan,
            order: self.order,
            live: self.live,
            total: self.total,
        });
        let (bins, cold_bins, gaps, result) = (a.bins.clone(), a.cold_bins.clone(), self.gaps.clone(), a.result.clone());
        self.h.publish(|s| {
            let e = s.extended.as_mut().expect("extended state is set before the schedule starts");
            e.bins = bins;
            e.cold_bins = cold_bins;
            e.gaps = gaps;
            e.result = result;
            json!({"type": "extended", "bins": e.bins, "cold_bins": e.cold_bins, "gaps": e.gaps, "result": e.result})
        });
        self.emitted = upto;
        a
    }
}

pub(crate) fn run(h: &RunHandle, cfg: &RunConfig, selected: &[Resolver], deps: &Deps) {
    let timeout = Duration::from_millis(QUERY_TIMEOUT_MS);
    let (gap_ms, workers) = (cfg.pace.gap_ms(), cfg.pace.workers());
    let bin_s = PER_BIN as f64 * gap_ms as f64 / 1000.0;

    // ---- Setup: the targets, then the names.
    h.phase("setup", 0);
    let set = Setup::new(cfg);
    let mut plan = bench::expand(set, cfg, selected, deps.client_v6, timeout, h);
    if cfg.aa && plan.len() == 1 {
        let mut twin = plan[0].clone();
        plan[0].0.variant = "A".into();
        twin.0.variant = "B".into();
        twin.0.idx = 1;
        plan.push(twin);
    }
    h.publish(|s| {
        s.targets = plan.iter().map(|(t, _)| t.clone()).collect();
        json!({"type": "targets", "targets": s.targets})
    });
    let source = namelist::bundled(deps.env.pointer("/vantage/geo/country").and_then(|c| c.as_str()));
    let seed = fastrand::u64(..1 << 53);
    let names = namelist::plan(cfg.samples as usize, &source, seed);
    let total = names.names.len();

    deps.measuring.store(true, Ordering::SeqCst);
    let xs = bench::open_targets(set, h, &plan, timeout);
    h.emit_targets();
    // One clock for the link samples and the lookups: counted from the run's start, sleep included.
    let started_at = h.with(|s| s.started_at);
    let origin_ns = clock::awake_total_ns().1.saturating_sub((now_ms() - started_at).max(0) as u64 * 1_000_000);
    let since_ms = move || (clock::awake_total_ns().1.saturating_sub(origin_ns) / 1_000_000).min(u32::MAX as u64) as u32;
    let pinger = (!link::disabled()).then(|| link::start(since_ms));

    // ---- Warm-up: one lookup on every connection, so none starts cold.
    let mut conns: Vec<Vec<Box<dyn Exchanger>>> = std::thread::scope(|sc| {
        let opening: Vec<_> = xs
            .into_iter()
            .zip(&plan)
            .map(|(x, (_, ep))| {
                sc.spawn(move || {
                    let mut v: Vec<Box<dyn Exchanger>> = x.into_iter().collect();
                    // A resolver holds state for every TCP or TLS connection, so those keep to the slower paces' count.
                    let cap = |ep: &net::Endpoint| {
                        if matches!(ep.transport, Transport::Udp) { workers } else { workers.min(bench::Pacing::Standard.workers()) }
                    };
                    while let Some(ep) = ep.as_ref().filter(|ep| !v.is_empty() && v.len() < cap(ep)) {
                        match net::open(ep, timeout) {
                            Ok(x) => v.push(x),
                            Err(_) => break,
                        }
                    }
                    v
                })
            })
            .collect();
        opening.into_iter().map(|j| j.join().unwrap_or_default()).collect()
    });
    h.phase("warmup", conns.iter().map(Vec::len).sum::<usize>() as u32);
    let answered: Vec<Vec<bool>> = std::thread::scope(|sc| {
        let asking: Vec<Vec<_>> = conns
            .iter_mut()
            .map(|v| {
                v.iter_mut()
                    .map(|x| {
                        sc.spawn(move || {
                            let ok = dns::encode_query(dns::new_id(), &QuerySpec::new(WARM_NAME, dns::TYPE_A))
                                .ok()
                                .is_some_and(|q| dns::ask_wire(x.as_mut(), &q, timeout).is_ok());
                            h.publish(|s| {
                                s.done += 1;
                                json!({"type": "warm", "samples": [], "done": s.done, "total": s.total})
                            });
                            ok
                        })
                    })
                    .collect()
            })
            .collect();
        asking.into_iter().map(|v| v.into_iter().map(|j| j.join().unwrap_or(false)).collect()).collect()
    });
    for (t, (v, ok)) in conns.iter_mut().zip(&answered).enumerate() {
        let had = !v.is_empty();
        let mut it = ok.iter();
        v.retain(|_| *it.next().unwrap_or(&false));
        if had && v.is_empty() {
            h.set_target(t, |t| t.fail("unreachable", "No answer to any warm-up query."));
        }
    }
    h.emit_targets();
    let live: Vec<bool> = conns.iter().map(|v| !v.is_empty()).collect();
    let live_n = live.iter().filter(|l| **l).count();
    if live_n < 2 || h.cancelled() {
        if live_n < 2 && !h.cancelled() {
            h.note("warn", "Fewer than two resolvers answered, so there is nothing to compare.".into(), None);
        }
        end_link(h, pinger);
        return;
    }

    // ---- The measured phase.
    let live_ids: Vec<u8> = (0..live.len()).filter(|&t| live[t]).map(|t| t as u8).collect();
    let order: Vec<Vec<u8>> = (0..total)
        .map(|_| {
            let mut o = live_ids.clone();
            fastrand::shuffle(&mut o);
            o
        })
        .collect();
    h.publish(|s| {
        s.extended = Some(ExtendedState {
            total: total as u32,
            per_bin: PER_BIN as u32,
            bin_s,
            cold_bin_s: bin_s * COLD_BINS as f64,
            gap_ms: gap_ms as u32,
            timeout_ms: QUERY_TIMEOUT_MS as u32,
            names: NamesUsed {
                source: source.kind,
                list_id: source.id.clone(),
                list_date: source.date.clone(),
                seed,
                common: names.touch.iter().filter(|&&k| k == 1).count() as u32,
                tail: names.touch.iter().filter(|&&k| k == 0).count() as u32,
                region: source.region,
                regional: source.regional.len().min(names.touch.iter().filter(|&&k| k == 0).count()) as u32,
            },
            plan: names.clone(),
            order: order.clone(),
            bins: Vec::new(),
            cold_bins: Vec::new(),
            gaps: Vec::new(),
            result: ExtResult::default(),
        });
        json!({"type": "extended_start", "extended": s.extended})
    });
    h.phase("extended", (live_n * total) as u32);
    let mut collector = Collector {
        h,
        bin_s,
        plan: &names,
        order: &order,
        live: &live,
        total,
        grid: vec![vec![None; live.len()]; total],
        sent_ns: Vec::new(),
        batch: Vec::new(),
        last_flush: Instant::now(),
        gaps: Vec::new(),
        windows: Vec::new(),
        done: 0,
        emitted: 0,
    };
    let keep_awake = power::KeepAwake::hold();
    let d = Drive { slot: Duration::from_millis(gap_ms), timeout, plan: &names, order: &order, cancel: &h.cancel, origin_ns };
    drive(&d, conns, watch(), &mut |ev| collector.on(ev));
    drop(keep_awake);

    // What was measured is kept, a cancel included.
    let a = collector.emit(collector.done);
    for t in a.shared_cache {
        let label = h.with(|s| format!("{} {}", s.targets[t].provider, s.targets[t].variant).trim().to_string());
        let id = h.with(|s| s.targets[t].resolver_id.clone());
        h.note(
            "warn",
            format!("{label} answered faster when another resolver had just been asked the same name, which suggests a shared cache. Its figures may flatter it."),
            Some(id),
        );
    }
    end_link(h, pinger);
    deps.measuring.store(false, Ordering::SeqCst);
}

/// Stops the link check and puts what it saw in the run, as the other tests do.
fn end_link(h: &RunHandle, pinger: Option<Result<link::LinkProbe, String>>) {
    let Some(pinger) = pinger else { return };
    let (value, noisy) = link::report(pinger.map(link::LinkProbe::stop));
    h.publish(|s| {
        s.link = Some(value);
        json!({"type": "link", "link": s.link})
    });
    if let Some(text) = noisy {
        h.note("warn", text, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Reply, XErr};
    use std::sync::Arc;

    fn sample(t: usize, q: usize, o: Outcome, ms: f64) -> Sample {
        Sample {
            t: t as u16,
            k: TestKind::Uncached,
            i: q as u32,
            us: (ms * 1000.0) as u32,
            o,
            at: (q * 300) as u32,
            h: 0,
            d: None,
            x: false,
        }
    }

    fn plan(n: usize) -> Plan {
        Plan { names: (0..n).map(|q| format!("q{q}.example.com")).collect(), touch: vec![0; n] }
    }

    fn grid(slots: usize, f: impl Fn(usize, usize) -> (Outcome, f64)) -> Vec<Vec<Option<Sample>>> {
        (0..slots)
            .map(|q| {
                (0..2)
                    .map(|t| {
                        let (o, ms) = f(q, t);
                        Some(sample(t, q, o, ms))
                    })
                    .collect()
            })
            .collect()
    }

    fn noise(q: usize) -> f64 {
        (q * 7919 % 13) as f64 * 0.1
    }

    fn run_on(g: &[Vec<Option<Sample>>], total: usize, order: &[Vec<u8>]) -> Analysis {
        analyze(&Input { bin_s: 15.0, grid: g, plan: &plan(total), order, live: &[true, true], total })
    }

    fn ordered(n: usize) -> Vec<Vec<u8>> {
        vec![vec![0, 1]; n]
    }

    fn simple(slots: usize, f: impl Fn(usize, usize) -> (Outcome, f64)) -> Analysis {
        run_on(&grid(slots, f), slots, &ordered(slots))
    }

    /// Four slots in five ask a common name; the fifth is a tail name.
    fn mixed(slots: usize, total: usize, f: impl Fn(usize, usize) -> (Outcome, f64)) -> Analysis {
        let plan = Plan {
            names: (0..total).map(|q| format!("q{q}.example.com")).collect(),
            touch: (0..total).map(|q| if q % 5 == 0 { 0 } else { 1 + (q % 8) as u8 }).collect(),
        };
        analyze(&Input { bin_s: 15.0, grid: &grid(slots, f), plan: &plan, order: &ordered(total), live: &[true, true], total })
    }

    /// A is quicker on common names and slower on tail names; common names are most of the run.
    fn a_leads_but_b_is_quicker_cold(q: usize, t: usize) -> (Outcome, f64) {
        let cold = q.is_multiple_of(5);
        (
            Outcome::Ok,
            match (t, cold) {
                (0, false) => 10.0,
                (0, true) => 40.0,
                _ => 30.0,
            },
        )
    }

    #[test]
    fn the_cold_ranking_uses_only_the_once_asked_names() {
        let a = mixed(1000, 1000, a_leads_but_b_is_quicker_cold);
        let r = &a.result;
        assert_eq!((r.rows[0].t, r.rows[0].p50), (0, Some(10.0)));
        assert_eq!(r.cold.iter().map(|c| c.t).collect::<Vec<_>>(), vec![1, 0]);
        assert_eq!((r.cold[0].sent, r.cold[0].p50), (200, Some(30.0)));
        assert_eq!(r.cold[0].call, Some("lead"));
        assert_eq!(r.cold[1].call, Some("slower"));
        assert_eq!(r.cold[1].vs.as_ref().unwrap().diff, 10.0);
        assert_eq!(r.cold_margin, 3.0);
    }

    #[test]
    fn cold_calls_wait_for_the_end_of_the_run() {
        let a = mixed(1000, 2000, a_leads_but_b_is_quicker_cold);
        let r = &a.result;
        assert!(!r.is_final);
        assert!(r.cold[1].vs.is_some());
        assert!(r.cold.iter().all(|c| c.call.is_none()));
    }

    #[test]
    fn too_few_once_asked_slots_give_no_cold_range() {
        // 90 of 450 slots are tail names.
        let a = mixed(450, 450, a_leads_but_b_is_quicker_cold);
        assert_eq!(a.result.cold.len(), 2);
        assert!(a.result.cold[1].vs.is_none() && a.result.cold.iter().all(|c| c.call.is_none()));
    }

    /// Every fifth slot is a tail name; the rest ask a common name, for the first time in the first eighth and again after.
    fn phased(total: usize, f: impl Fn(usize, usize) -> (Outcome, f64)) -> (Analysis, Plan) {
        let plan = Plan {
            names: (0..total).map(|q| format!("q{q}.example.com")).collect(),
            touch: (0..total)
                .map(|q| {
                    if q.is_multiple_of(5) {
                        0
                    } else if q < total / 8 {
                        1
                    } else {
                        2
                    }
                })
                .collect(),
        };
        let a = analyze(&Input { bin_s: 15.0, grid: &grid(total, f), plan: &plan, order: &ordered(total), live: &[true, true], total });
        (a, plan)
    }

    #[test]
    fn the_headline_leaves_out_first_asks() {
        let touch = |q: usize| {
            if q.is_multiple_of(5) {
                0
            } else if q < 125 {
                1
            } else {
                2
            }
        };
        let (a, plan) = phased(1000, |q, t| (Outcome::Ok, if t == 0 { if touch(q) == 1 { 300.0 } else { 10.0 } } else { 30.0 }));
        let counted = plan.touch.iter().filter(|&&k| k != 1).count() as u32;
        let row = a.result.rows.iter().find(|r| r.t == 0).unwrap();
        assert_eq!((row.sent, row.p50), (counted, Some(10.0)));
        assert_eq!(a.result.rows[0].t, 0);
        let first = a.result.groups.iter().find(|g| g.key == "common_first").unwrap();
        assert_eq!(first.count, plan.touch.iter().filter(|&&k| k == 1).count() as u32);
        assert_eq!(first.r[0].p50, Some(300.0));
    }

    #[test]
    fn repeat_cells_leave_out_first_asks_and_tail_names() {
        let (a, plan) = phased(1000, |q, _| (Outcome::Ok, 20.0 + noise(q)));
        assert_eq!(a.bins[0].rep.len(), 2);
        assert_eq!(a.bins[0].rep[0].sent, 0);
        for b in [4, 10] {
            let later = (b * PER_BIN..(b + 1) * PER_BIN).filter(|&q| plan.touch[q] >= 2).count() as u32;
            assert_eq!(a.bins[b].rep[1].sent, later);
            assert_eq!(a.bins[b].r[1].sent, 50);
            let counted = (b * PER_BIN..(b + 1) * PER_BIN).filter(|&q| plan.touch[q] != 1).count() as u32;
            assert_eq!(a.bins[b].all[1].sent, counted);
        }
        let firsts = plan.touch.iter().take(a.bins.len() * PER_BIN).filter(|&&k| k == 1).count() as u32;
        assert!(firsts > 0);
        assert_eq!(a.bins.iter().map(|b| b.all[1].sent).sum::<u32>() + firsts, a.bins.iter().map(|b| b.r[1].sent).sum::<u32>());
    }

    #[test]
    fn first_cells_hold_the_first_asks_alone() {
        let (a, plan) = phased(1000, |q, _| (Outcome::Ok, 20.0 + noise(q)));
        for b in [0, 4, 10] {
            let firsts = (b * PER_BIN..(b + 1) * PER_BIN).filter(|&q| plan.touch[q] == 1).count() as u32;
            assert_eq!(a.bins[b].first[1].sent, firsts);
            assert_eq!(a.bins[b].first[1].sent + a.bins[b].all[1].sent, a.bins[b].r[1].sent);
        }
        assert!(a.bins[0].first[0].sent > 0);
    }

    #[test]
    fn cold_bins_hold_the_once_asked_lookups() {
        let (a, _) = phased(1000, |q, _| (Outcome::Ok, 20.0 + noise(q)));
        assert_eq!(a.cold_bins.len(), a.bins.len().div_ceil(COLD_BINS));
        for t in 0..2 {
            let row = a.result.cold.iter().find(|r| r.t == t).unwrap();
            assert_eq!(a.cold_bins.iter().map(|b| b.r[t].sent).sum::<u32>(), row.sent);
        }
        assert!(a.cold_bins.iter().all(|b| b.rep.is_empty() && b.all.is_empty() && b.first.is_empty() && !b.removed));
        assert_eq!((a.cold_bins[1].at, a.cold_bins[1].end), (a.bins[5].at, a.bins[9].end));
    }

    #[test]
    fn a_cold_bin_is_removed_when_all_its_bins_are() {
        let (a, _) = phased(1000, |q, _| if q < 250 { (Outcome::Timeout, 0.0) } else { (Outcome::Ok, 20.0) });
        assert!(a.cold_bins[0].removed && !a.cold_bins[1].removed);
        assert_eq!(a.cold_bins[0].r[0].sent, 0);
    }

    #[test]
    fn same_resolver_twice_is_a_tie() {
        let a = simple(1000, |q, t| (Outcome::Ok, 20.0 + noise(q + t * 5)));
        let r = &a.result;
        assert!(r.is_final && !r.no_verdict);
        assert_eq!(r.rows[0].call, Some("lead"));
        assert_eq!(r.rows[1].call, Some("tie"));
        assert!(r.rows[1].vs.as_ref().unwrap().diff.abs() < 0.5);
    }

    #[test]
    fn known_gap_is_slower_with_the_known_range() {
        // Ten bins make ten batches of one. Leaving one out moves the gap to 9 or 11 ms, so the ten have mean 10 and a standard error of 3;
        // t for one comparison is 2.262.
        let a = simple(500, |q, t| {
            (
                Outcome::Ok,
                if t == 0 {
                    20.0
                } else if (q / PER_BIN).is_multiple_of(2) {
                    29.0
                } else {
                    31.0
                },
            )
        });
        let v = a.result.rows[1].vs.clone().unwrap();
        assert_eq!(a.result.rows[1].call, Some("slower"));
        assert_eq!(v, Vs { diff: 10.0, lo: 3.21, hi: 16.79 });
        assert_eq!(a.result.margin, 3.0);
    }

    #[test]
    fn a_gap_that_moves_by_hundredths_still_has_a_range() {
        // Leaving a stretch out moves the gap by 0.02 ms, which medians rounded to a tenth would hide.
        let a = simple(500, |q, t| {
            (
                Outcome::Ok,
                if t == 0 {
                    20.0
                } else if (q / PER_BIN).is_multiple_of(2) {
                    29.99
                } else {
                    30.01
                },
            )
        });
        let v = a.result.rows[1].vs.clone().unwrap();
        assert!(v.lo < v.hi, "{v:?}");
    }

    #[test]
    fn the_last_bins_are_left_out_in_turn_too() {
        // Fourteen bins, six at 29 ms and eight at 31: the median moves only when two of the eight go together,
        // as they do in the last stretch.
        let second = |q: usize| if q / PER_BIN < 6 { 29.0 } else { 31.0 };
        let a = simple(700, |q, t| (Outcome::Ok, if t == 0 { 20.0 } else { second(q) }));
        let v = a.result.rows[1].vs.clone().unwrap();
        assert!(v.lo < v.hi, "{v:?}");
    }

    #[test]
    fn wide_swings_are_unclear() {
        let a = simple(500, |q, t| {
            (
                Outcome::Ok,
                if t == 0 {
                    40.0
                } else if (q / PER_BIN).is_multiple_of(2) {
                    60.0
                } else {
                    20.0
                },
            )
        });
        assert_eq!(a.result.rows[1].call, Some("unclear"));
    }

    #[test]
    fn a_dead_bin_is_removed_and_counted_out() {
        let a = simple(1000, |q, _| if (150..200).contains(&q) { (Outcome::Timeout, 0.0) } else { (Outcome::Ok, 20.0) });
        assert!(a.bins[3].removed);
        assert!(!a.result.no_verdict);
        assert_eq!(a.result.removed_s, 15.0);
        assert_eq!(a.result.rows[0].sent, 950);
        assert_eq!(a.result.rows[0].lost, 0);
    }

    #[test]
    fn too_many_removed_bins_leave_no_verdict() {
        let a = simple(1000, |q, _| if q / PER_BIN < 5 { (Outcome::Timeout, 0.0) } else { (Outcome::Ok, 20.0) });
        assert!(a.result.no_verdict);
        assert!(a.result.rows.iter().all(|r| r.call.is_none()));
    }

    #[test]
    fn a_name_nobody_resolved_is_dropped_for_all() {
        let a = simple(1000, |q, _| if q == 10 { (Outcome::Nxdomain, 20.0) } else { (Outcome::Ok, 20.0) });
        assert_eq!(a.result.dropped_names, 1);
        assert!(a.result.rows.iter().all(|r| r.sent == 999 && r.blocked == 0));
    }

    #[test]
    fn a_name_one_resolver_blocked_is_out_of_its_timing_only() {
        let a = simple(1000, |q, t| if q == 20 && t == 1 { (Outcome::Blocked, 20.0) } else { (Outcome::Ok, 20.0) });
        let row = |t| a.result.rows.iter().find(|r| r.t == t).unwrap();
        assert_eq!((row(0).sent, row(0).blocked), (1000, 0));
        assert_eq!((row(1).sent, row(1).blocked), (999, 1));
    }

    #[test]
    fn stopped_early_has_figures_but_no_call() {
        let g = grid(600, |q, t| (Outcome::Ok, 20.0 + 10.0 * t as f64 + noise(q)));
        let a = run_on(&g, 1000, &ordered(1000));
        let r = &a.result;
        assert!(!r.is_final);
        assert!(r.rows.iter().all(|r| r.call.is_none()));
        assert!(r.rows[1].vs.is_some());
        assert_eq!(a.bins.len(), 12);
    }

    #[test]
    fn fewer_than_500_kept_slots_give_no_range() {
        let a = simple(400, |_, t| (Outcome::Ok, 20.0 + 10.0 * t as f64));
        assert!(a.result.rows[1].vs.is_none() && a.result.rows[1].call.is_none());
    }

    #[test]
    fn a_resolver_losing_more_is_flagged() {
        let a = simple(1000, |q, t| match (t, q % 50) {
            (0, 7) => (Outcome::Timeout, 0.0),
            (0, _) => (Outcome::Ok, 19.0),
            _ => (Outcome::Ok, 20.0),
        });
        assert_eq!(a.result.rows[0].t, 0);
        assert_eq!(a.result.rows[0].loss_pct, 2.0);
        assert!(a.result.loss_flag);
    }

    #[test]
    fn bins_add_up_to_the_table() {
        let a = simple(1000, |q, t| if q % 37 == 0 && t == 1 { (Outcome::Timeout, 0.0) } else { (Outcome::Ok, 20.0 + noise(q)) });
        for r in &a.result.rows {
            let kept = a.bins.iter().filter(|b| !b.removed);
            assert_eq!(kept.clone().map(|b| b.r[r.t].sent).sum::<u32>(), r.sent);
            assert_eq!(kept.map(|b| b.r[r.t].lost).sum::<u32>(), r.lost);
        }
        assert!(a.result.ecdf.r[0].last().is_some_and(|v| *v == 1.0));
    }

    #[test]
    fn an_excused_loss_counts_as_nothing() {
        let mut g = grid(1000, |q, _| (Outcome::Ok, 20.0 + noise(q)));
        let mut s = sample(1, 5, Outcome::Timeout, 0.0);
        s.x = true;
        g[5][1] = Some(s);
        let a = run_on(&g, 1000, &ordered(1000));
        let row = a.result.rows.iter().find(|r| r.t == 1).unwrap();
        assert_eq!((row.sent, row.lost), (999, 0));
    }

    #[test]
    fn a_faster_answer_when_asked_later_is_flagged() {
        let order: Vec<Vec<u8>> = (0..1000).map(|q| if q % 2 == 0 { vec![1, 0] } else { vec![0, 1] }).collect();
        let g = grid(1000, |q, t| {
            (
                Outcome::Ok,
                if t == 0 {
                    20.0
                } else if q % 2 == 0 {
                    50.0
                } else {
                    10.0
                },
            )
        });
        let a = run_on(&g, 1000, &order);
        assert_eq!(a.shared_cache, vec![1]);
    }

    #[test]
    fn state_serialises_with_the_documented_keys() {
        let g = grid(50, |q, t| (Outcome::Ok, 20.0 + 5.0 * t as f64 + noise(q)));
        let a = run_on(&g, 50, &ordered(50));
        let e = ExtendedState {
            total: 50,
            per_bin: 50,
            bin_s: 15.0,
            cold_bin_s: 75.0,
            gap_ms: 300,
            timeout_ms: 2000,
            names: NamesUsed {
                source: "bundled",
                list_id: "pool-2026-11-01".into(),
                list_date: "2026-11-01".into(),
                seed: 7,
                common: 5,
                tail: 40,
                region: None,
                regional: 0,
            },
            plan: plan(50),
            order: ordered(50),
            bins: a.bins,
            cold_bins: a.cold_bins,
            gaps: vec![(1000, 5000, "sleep")],
            result: a.result,
        };
        let v = serde_json::to_value(&e).unwrap();
        for k in [
            "total",
            "per_bin",
            "bin_s",
            "cold_bin_s",
            "gap_ms",
            "timeout_ms",
            "names",
            "plan",
            "order",
            "bins",
            "cold_bins",
            "gaps",
            "result",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert!(v["bins"][0]["rep"].is_array());
        assert_eq!(v["cold_bin_s"], 75.0);
        assert!(v["cold_bins"][0].get("rep").is_none());
        assert!(v["cold_bins"][0].get("all").is_none());
        assert_eq!(v["cold_bins"][0]["r"][0]["sent"], 50);
        for k in ["final", "no_verdict", "margin", "loss_flag", "removed_s", "dropped_names", "rows", "ecdf", "groups"] {
            assert!(v["result"].get(k).is_some(), "{k}");
        }
        assert_eq!(v["gaps"][0], json!([1000, 5000, "sleep"]));
        assert_eq!(v["bins"][0]["r"][0]["sent"], 50);
        assert!(v["result"]["rows"][0]["vs"].is_null());
        let mut bin = v["bins"][0].clone();
        bin["at"] = json!(0);
        println!(
            "{}",
            json!({"type": "extended", "bins": [bin], "gaps": v["gaps"], "result": {"final": v["result"]["final"], "rows": v["result"]["rows"]}})
        );
    }

    /// Answers at once, or never (a timeout after `lag`), and logs when each query came.
    struct Fake {
        t: usize,
        lag: Option<Duration>,
        log: Arc<Mutex<Vec<(usize, Instant)>>>,
    }

    impl Exchanger for Fake {
        fn exchange(&mut self, query: &[u8], _: Duration) -> Result<Reply, XErr> {
            self.log.lock().unwrap().push((self.t, Instant::now()));
            if let Some(lag) = self.lag {
                std::thread::sleep(lag);
                return Err(XErr::Timeout);
            }
            let mut bytes = query.to_vec();
            bytes[2] |= 0x80;
            bytes[7] = 1;
            Ok(Reply { bytes, rtt_ns: 5_000_000, user_rtt_ns: 5_000_000, kernel_stamp: false, ttl: None })
        }
    }

    fn run_fake(slots: usize, pace: impl FnMut() -> Pace + Send) -> (Vec<(usize, Instant)>, Vec<Sample>, usize) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let conns: Vec<Vec<Box<dyn Exchanger>>> = (0..2)
            .map(|t| {
                (0..8)
                    .map(|_| {
                        Box::new(Fake { t, lag: (t == 1).then(|| Duration::from_millis(200)), log: log.clone() }) as Box<dyn Exchanger>
                    })
                    .collect()
            })
            .collect();
        let order: Vec<Vec<u8>> = (0..slots).map(|q| if q % 2 == 0 { vec![0, 1] } else { vec![1, 0] }).collect();
        let p = plan(slots);
        let cancel = AtomicBool::new(false);
        let d = Drive {
            slot: Duration::from_millis(30),
            timeout: Duration::from_millis(500),
            plan: &p,
            order: &order,
            cancel: &cancel,
            origin_ns: clock::awake_total_ns().1,
        };
        let (mut samples, mut gaps) = (Vec::new(), 0);
        drive(&d, conns, pace, &mut |ev| match ev {
            Ev::Msg(Msg::Done { sample, .. }) => samples.push(sample),
            Ev::Msg(Msg::Gap { .. }) => gaps += 1,
            Ev::Tick => {}
        });
        let log = log.lock().unwrap().clone();
        (log, samples, gaps)
    }

    #[test]
    fn timeouts_do_not_delay_later_sends() {
        let (log, samples, _) = run_fake(20, || Pace::Go);
        assert_eq!(samples.len(), 40);
        assert_eq!(samples.iter().filter(|s| s.o == Outcome::Timeout).count(), 20);
        assert!(samples.iter().filter(|s| s.t == 0).all(|s| s.o == Outcome::Ok));
        // Target 0's k-th send is due at slot k, plus half a slot when it goes second. A busy machine can make any one
        // send late; a timeout that held up the schedule would make every later one late.
        let mut t0: Vec<Instant> = log.iter().filter(|(t, _)| *t == 0).map(|(_, at)| *at).collect();
        t0.sort();
        let first = t0[0];
        let off: Vec<String> = t0
            .iter()
            .enumerate()
            .filter_map(|(q, at)| {
                let due = Duration::from_millis(30 * q as u64 + if q % 2 == 0 { 0 } else { 15 });
                let got = at.duration_since(first);
                (got.abs_diff(due) >= Duration::from_millis(40)).then(|| format!("send {q}: due {due:?}, got {got:?}"))
            })
            .collect();
        assert!(off.len() <= 3, "{off:?}");
    }

    #[test]
    fn a_hold_waits_and_a_resume_records_one_gap() {
        let mut n = 0;
        let (_, samples, gaps) = run_fake(6, move || {
            n += 1;
            match n {
                3..=5 => Pace::Hold,
                6 => Pace::Resumed { since_ns: 1, until_ns: 100_000_000, cause: "sleep" },
                _ => Pace::Go,
            }
        });
        assert_eq!(samples.len(), 12);
        assert_eq!(gaps, 1);
    }

    #[test]
    fn a_short_stall_is_not_caught_up_in_a_burst() {
        let mut n = 0;
        let woke = Arc::new(Mutex::new(None));
        let w = woke.clone();
        let (log, samples, gaps) = run_fake(8, move || {
            n += 1;
            if n == 3 {
                std::thread::sleep(Duration::from_millis(1100));
                *w.lock().unwrap() = Some(Instant::now());
            }
            Pace::Go
        });
        assert_eq!((samples.len(), gaps), (16, 0));
        // The stall came before the second send of slot 0. Carried on from there, target 0's last send (slot 7, half a
        // slot in) is due 210 ms after it; caught up in a burst, it would go at once. No send goes before it is due, so a
        // busy machine can only make it later.
        let woke = woke.lock().unwrap().expect("the stall ran");
        let last = log.iter().filter(|(t, _)| *t == 0).map(|(_, at)| *at).max().unwrap();
        let after = last.duration_since(woke);
        assert!(after >= Duration::from_millis(150), "last send {after:?} after the stall");
    }

    #[test]
    fn a_two_resolver_minimum_is_enforced() {
        let cfg = RunConfig::with_resolvers(vec!["nobody".into()]);
        assert!(check(&cfg, &[]).is_err());
    }
}
