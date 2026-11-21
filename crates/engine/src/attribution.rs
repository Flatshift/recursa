//! The pure pieces of the monitor's drop attribution: when a cycle may count at all (sleep, stalls and the
//! settling after them), which resolvers are one operator, and what a failed cycle is down to. No clock, OS,
//! network or store calls: the monitor loop feeds readings in and acts on what comes out.

use crate::catalog::{self, Resolver};
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;

const S: u64 = 1_000_000_000;
/// The loop's tick.
const TICK_NS: u64 = 10 * S;
/// Total time running ahead of awake time by this much is a sleep.
const SLEEP_NS: u64 = S;
/// A tick this much later than the tick length is a stall.
const STALL_LATE_NS: u64 = 5 * S;
/// No cycle starts for this long after a gap.
const HOLD_NS: u64 = 15 * S;
/// A first cycle after a hold in which nothing answers is retried until this long after the hold.
const SETTLE_NS: u64 = 120 * S;
/// A will-sleep with no wake after it is taken as a lost wake event once the awake clock has run this long past it.
const LOST_WAKE_NS: u64 = 30 * S;

/// Why a failed cycle counts as it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// Several independent operators failed and none answered.
    Shared,
    /// Several failed while another answered.
    Unclear,
    /// This resolver or its path.
    Isolated,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Shared => "shared",
            Cause::Unclear => "unclear",
            Cause::Isolated => "isolated",
        }
    }

    pub fn parse(s: &str) -> Option<Cause> {
        match s {
            "shared" => Some(Cause::Shared),
            "unclear" => Some(Cause::Unclear),
            "isolated" => Some(Cause::Isolated),
            _ => None,
        }
    }
}

/// Why no cycle ran for a while.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapCause {
    Sleep,
    Stall,
}

impl GapCause {
    pub fn as_str(self) -> &'static str {
        match self {
            GapCause::Sleep => "sleep",
            GapCause::Stall => "stall",
        }
    }
}

/// The clocks and power stamps at one moment, all in ns on the monotonic clocks of `clock::awake_total_ns`
/// (`awake` stops during sleep, `total` doesn't); a stamp of 0 is none seen.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Reading {
    pub awake: u64,
    pub total: u64,
    pub slept_at: u64,
    pub woke_at: u64,
}

/// A stretch in which the monitor wasn't measuring, from `since` (on the total clock) until a cycle counts again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub cause: GapCause,
    pub since: u64,
}

/// What to do with a finished cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Record it; it ends the gap, if one was open.
    Keep(Option<Gap>),
    /// Throw its rows away.
    Void,
}

/// Decides when a cycle may start and whether a finished one counts, from readings fed once per tick and
/// around each cycle.
#[derive(Debug, Default)]
pub(crate) struct GapWatch {
    prev: Option<Reading>,
    seen_slept: u64,
    seen_woke: u64,
    /// No cycle before this (total clock).
    hold_until: u64,
    /// Until this, a cycle in which nothing answers is void (total clock).
    settle_until: u64,
    /// A will-sleep with no wake after it: (its stamp, the awake clock when first seen).
    pending: Option<(u64, u64)>,
    /// A will-sleep stamp given up on as a lost wake.
    lost: u64,
    gap: Option<Gap>,
}

impl GapWatch {
    /// A tick of the loop. Whether a cycle may start now.
    pub(crate) fn tick(&mut self, now: Reading) -> bool {
        self.observe(now, true);
        self.ready(now.total)
    }

    /// A reading after work that took long (a cycle's lookups): like a tick, but being late is no stall.
    pub(crate) fn rebase(&mut self, now: Reading) -> bool {
        self.observe(now, false);
        self.ready(now.total)
    }

    /// A gap is open: the next counted cycle ends it.
    pub(crate) fn in_gap(&self) -> bool {
        self.gap.is_some()
    }

    /// A cycle ended at `after`, and `answered` says whether any monitored resolver answered in it. It is measured
    /// from the last tick or rebase, so a sleep or power event just before its first lookup counts against it too.
    pub(crate) fn end_cycle(&mut self, after: Reading, answered: bool) -> Verdict {
        let before = self.prev.replace(after).unwrap_or(after);
        let slept = after.total.saturating_sub(before.total).saturating_sub(after.awake.saturating_sub(before.awake));
        let slept_event = after.slept_at != before.slept_at;
        let woke_event = after.woke_at != before.woke_at;
        if slept >= SLEEP_NS || slept_event || woke_event {
            self.seen_slept = after.slept_at;
            self.seen_woke = after.woke_at;
            let since = if slept_event && after.slept_at != 0 { after.slept_at } else { before.total };
            self.start_gap(GapCause::Sleep, since, if woke_event { after.woke_at } else { after.total });
            return Verdict::Void;
        }
        if self.gap.is_some() {
            if answered || after.total >= self.settle_until {
                return Verdict::Keep(self.gap.take());
            }
            // Nothing answered yet: the network may still be coming back. Try again after a hold.
            self.hold_until = after.total + HOLD_NS;
            return Verdict::Void;
        }
        Verdict::Keep(None)
    }

    fn ready(&self, total: u64) -> bool {
        self.pending.is_none() && total >= self.hold_until
    }

    fn observe(&mut self, now: Reading, check_late: bool) {
        let first = self.prev.is_none();
        let prev = self.prev.replace(now).unwrap_or(now);
        if first {
            self.seen_slept = now.slept_at;
            self.seen_woke = now.woke_at;
        }
        let d_total = now.total.saturating_sub(prev.total);
        let slept = d_total.saturating_sub(now.awake.saturating_sub(prev.awake));
        let late = d_total.saturating_sub(TICK_NS);
        let new_sleep = now.slept_at != self.seen_slept;
        let new_wake = now.woke_at != self.seen_woke;
        self.seen_slept = now.slept_at;
        self.seen_woke = now.woke_at;
        let cause = if new_sleep || new_wake || slept >= SLEEP_NS {
            Some(GapCause::Sleep)
        } else if check_late && late > STALL_LATE_NS {
            Some(GapCause::Stall)
        } else {
            None
        };
        if let Some(cause) = cause {
            let since = if new_sleep && now.slept_at != 0 { now.slept_at } else { prev.total };
            self.start_gap(cause, since, if new_wake { now.woke_at } else { now.total });
        }
        if now.slept_at > now.woke_at && now.slept_at != self.lost {
            let p = match self.pending {
                Some(p) if p.0 == now.slept_at => p,
                _ => (now.slept_at, now.awake),
            };
            if now.awake.saturating_sub(p.1) >= LOST_WAKE_NS {
                // Awake and running for a while with no wake event: take it as lost.
                self.pending = None;
                self.lost = now.slept_at;
                self.start_gap(GapCause::Sleep, now.slept_at, now.total);
            } else {
                self.pending = Some(p);
            }
        } else {
            self.pending = None;
        }
    }

    fn start_gap(&mut self, cause: GapCause, since: u64, hold_from: u64) {
        let g = self.gap.get_or_insert(Gap { cause, since });
        if cause == GapCause::Sleep {
            g.cause = cause;
        }
        g.since = g.since.min(since);
        self.hold_until = self.hold_until.max(hold_from + HOLD_NS);
        self.settle_until = self.hold_until + SETTLE_NS;
    }
}

/// Where a resolver's lookups reach authoritative servers from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Egress {
    pub addr: IpAddr,
    pub asn: Option<u32>,
}

/// Which operator each resolver belongs to, and whether it counts toward telling operators apart.
#[derive(Debug, PartialEq)]
pub(crate) struct Operators {
    /// Per resolver, in input order: an operator number, equal for resolvers of one operator.
    pub op: Vec<usize>,
    /// A resolver with a public address, or a private one whose egress is known.
    pub independent: Vec<bool>,
}

impl Operators {
    /// Distinct operators among the independent resolvers.
    pub(crate) fn independent_count(&self) -> usize {
        self.op.iter().zip(&self.independent).filter(|(_, i)| **i).map(|(o, _)| *o).collect::<BTreeSet<_>>().len()
    }

    /// Distinct operators among all the resolvers, for when no egress is known and independence can't be judged.
    pub(crate) fn count(&self) -> usize {
        self.op.iter().collect::<BTreeSet<_>>().len()
    }
}

/// Groups `resolvers` into operators. Merging errs toward fewer operators, which can only turn a shared verdict into
/// unclear or isolated. Two resolvers are one operator when they share a provider (not for custom entries or unnamed
/// system resolvers, which carry generic names), when a custom or system entry has an address `catalog` lists under
/// the other's provider, or when their egress addresses sit in one AS (or are equal when the AS is unknown).
pub(crate) fn operators(resolvers: &[Resolver], catalog: &[Resolver], egress: &HashMap<String, Egress>) -> Operators {
    let provider: Vec<Option<String>> = resolvers
        .iter()
        .map(|r| {
            let generic = r.group == "custom" || r.group == "system" && r.custom_id.is_none();
            if !generic {
                return Some(r.provider.to_lowercase());
            }
            let listed = catalog.iter().find(|c| r.ipv4.iter().any(|a| c.ipv4.contains(a)) || r.ipv6.iter().any(|a| c.ipv6.contains(a)));
            listed.map(|c| c.provider.to_lowercase())
        })
        .collect();
    let same = |i: usize, j: usize| {
        if provider[i].is_some() && provider[i] == provider[j] {
            return true;
        }
        match (egress.get(&resolvers[i].id), egress.get(&resolvers[j].id)) {
            (Some(a), Some(b)) => match (a.asn, b.asn) {
                (Some(x), Some(y)) => x == y,
                _ => a.addr == b.addr,
            },
            _ => false,
        }
    };
    let mut op: Vec<usize> = (0..resolvers.len()).collect();
    for i in 0..resolvers.len() {
        for j in i + 1..resolvers.len() {
            if op[i] != op[j] && same(i, j) {
                let (from, to) = (op[j], op[i]);
                op.iter_mut().filter(|o| **o == from).for_each(|o| *o = to);
            }
        }
    }
    let independent =
        resolvers.iter().map(|r| r.any_ip().is_none_or(|ip| !catalog::is_private(&ip)) || egress.contains_key(&r.id)).collect();
    Operators { op, independent }
}

/// The cause of each unanswered resolver in one cycle, from (operator, independent, answered) per resolver; answered
/// ones get none. An operator is answered when any of its independent resolvers is. Two or more independent operators
/// unanswered is shared when no independent operator answered, unclear when one did; anything else is isolated.
pub(crate) fn attribute(cycle: &[(usize, bool, bool)]) -> Vec<Option<Cause>> {
    let answered: BTreeSet<usize> = cycle.iter().filter(|(_, ind, ok)| *ind && *ok).map(|(op, _, _)| *op).collect();
    let failed: BTreeSet<usize> = cycle.iter().filter(|(op, ind, _)| *ind && !answered.contains(op)).map(|(op, _, _)| *op).collect();
    let cause = match (failed.len(), answered.len()) {
        (0 | 1, _) => Cause::Isolated,
        (_, 0) => Cause::Shared,
        _ => Cause::Unclear,
    };
    cycle.iter().map(|(_, _, ok)| (!ok).then_some(cause)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn secs(s: u64) -> u64 {
        s * S
    }

    /// Both clocks in seconds, no power stamps.
    fn at(awake: u64, total: u64) -> Reading {
        Reading { awake: secs(awake), total: secs(total), ..Default::default() }
    }

    #[test]
    fn normal_ticks_make_no_gap() {
        let mut w = GapWatch::default();
        for t in (100..200).step_by(10) {
            assert!(w.tick(at(t, t)), "{t}");
            assert!(!w.in_gap());
        }
    }

    #[test]
    fn a_jump_in_total_alone_is_a_sleep_held_for_fifteen_seconds() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert!(w.tick(at(110, 110)));
        // Ten minutes pass while the awake clock stands still.
        assert!(!w.tick(at(110, 710)));
        assert!(w.in_gap());
        assert!(!w.tick(at(120, 720)));
        assert!(w.tick(at(130, 730)));
        // The gap runs from the last good tick to the first counted cycle.
        assert_eq!(w.end_cycle(at(131, 731), true), Verdict::Keep(Some(Gap { cause: GapCause::Sleep, since: secs(110) })));
        assert!(!w.in_gap());
        assert_eq!(w.end_cycle(at(141, 741), true), Verdict::Keep(None));
    }

    #[test]
    fn a_wake_event_with_both_clocks_flat_is_a_sleep_held_from_the_wake() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        let mut t = at(110, 110);
        (t.slept_at, t.woke_at) = (secs(102), secs(105));
        // Held until 105 + 15 = 120, not 110 + 15.
        assert!(!w.tick(t));
        assert!(w.tick(Reading { awake: secs(120), total: secs(120), ..t }));
        let after = Reading { awake: secs(121), total: secs(121), ..t };
        assert_eq!(w.end_cycle(after, true), Verdict::Keep(Some(Gap { cause: GapCause::Sleep, since: secs(102) })));
    }

    #[test]
    fn a_jump_in_both_clocks_is_a_stall() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert!(w.tick(at(110, 110)));
        assert!(!w.tick(at(150, 150)));
        assert!(!w.tick(at(160, 160)));
        assert!(w.tick(at(170, 170)));
        assert_eq!(w.end_cycle(at(172, 172), true), Verdict::Keep(Some(Gap { cause: GapCause::Stall, since: secs(110) })));
    }

    #[test]
    fn no_cycle_between_a_will_sleep_and_its_wake() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        let mut t = at(110, 110);
        t.slept_at = secs(105);
        assert!(!w.tick(t));
        // Hours pass asleep; the hold is long over but no wake was seen.
        assert!(!w.tick(Reading { total: secs(4000), ..t }));
        let t = Reading { awake: secs(120), total: secs(4010), woke_at: secs(4005), ..t };
        assert!(!w.tick(t));
        assert!(w.tick(Reading { awake: secs(130), total: secs(4020), ..t }));
    }

    #[test]
    fn a_lost_wake_event_does_not_block_forever() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        let will_sleep = |awake: u64| Reading { awake: secs(awake), total: secs(awake), slept_at: secs(105), woke_at: 0 };
        // Awake and ticking with a will-sleep and no wake: blocked while the awake clock has run under 30 s since first seen.
        assert!(!w.tick(will_sleep(110)));
        assert!(!w.tick(will_sleep(120)));
        assert!(!w.tick(will_sleep(130)));
        // 30 s on: taken as awake, held 15 s from this tick.
        assert!(!w.tick(will_sleep(140)));
        assert!(!w.tick(will_sleep(150)));
        assert!(w.tick(will_sleep(160)));
        assert_eq!(w.end_cycle(will_sleep(161), true), Verdict::Keep(Some(Gap { cause: GapCause::Sleep, since: secs(105) })));
    }

    #[test]
    fn a_cycle_that_slept_or_saw_a_power_event_is_void() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert_eq!(w.end_cycle(at(150, 700), true), Verdict::Void);
        assert!(w.in_gap());
        assert!(!w.tick(at(160, 710)));
        assert!(w.tick(at(170, 720)));
        // A wake stamped inside a cycle with both clocks flat.
        let mut after = at(171, 721);
        after.woke_at = secs(721);
        assert_eq!(w.end_cycle(after, true), Verdict::Void);
        assert!(!w.tick(Reading { awake: secs(181), total: secs(731), ..after }));
    }

    #[test]
    fn a_wake_stamped_between_the_tick_and_the_cycle_voids_it() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        // Both clocks flat and the stamp already there when the cycle starts: only the last tick shows it is new.
        let mut after = at(101, 101);
        after.woke_at = secs(100);
        assert_eq!(w.end_cycle(after, true), Verdict::Void);
        assert!(w.in_gap());
    }

    #[test]
    fn a_long_cycle_with_no_sleep_is_kept() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert_eq!(w.end_cycle(at(150, 150), false), Verdict::Keep(None));
        // The next tick is ten seconds after the cycle, not late.
        assert!(w.tick(at(160, 160)));
        assert!(!w.in_gap());
    }

    #[test]
    fn rebasing_after_slow_work_is_not_a_stall() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert!(w.rebase(at(130, 130)));
        assert!(w.tick(at(140, 140)));
        assert!(!w.in_gap());
    }

    #[test]
    fn nothing_answering_after_the_hold_is_retried_until_the_settle_limit() {
        // The awake clock is 500 s behind the total one from the sleep on.
        let r = |total: u64| at(total - 500, total);
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        // Asleep to 600: held to 615, settling to 735.
        assert!(!w.tick(r(600)));
        assert!(!w.tick(r(610)));
        assert!(w.tick(r(620)));
        // Each cycle takes 5 s and hears nothing; the retry is the first tick 15 s after it ends.
        for start in [620, 640, 660, 680, 700, 720] {
            assert_eq!(w.end_cycle(r(start + 5), false), Verdict::Void, "{start}");
            assert!(!w.tick(r(start + 10)), "{start}");
            assert!(w.tick(r(start + 20)), "{start}");
        }
        // Ended at 745, past 735: counted as usual, and it ends the gap.
        assert_eq!(w.end_cycle(r(745), false), Verdict::Keep(Some(Gap { cause: GapCause::Sleep, since: secs(100) })));
        assert_eq!(w.end_cycle(r(751), false), Verdict::Keep(None));
    }

    #[test]
    fn something_answering_ends_the_settle_at_once() {
        let mut w = GapWatch::default();
        assert!(w.tick(at(100, 100)));
        assert!(!w.tick(at(100, 600)));
        assert!(!w.tick(at(110, 610)));
        assert!(w.tick(at(120, 620)));
        assert_eq!(w.end_cycle(at(122, 622), false), Verdict::Void);
        assert!(!w.tick(at(130, 630)));
        assert!(w.tick(at(140, 640)));
        assert!(matches!(w.end_cycle(at(141, 641), true), Verdict::Keep(Some(_))));
    }

    fn res(id: &str, provider: &str, group: &str, addr: &str) -> Resolver {
        let mut r = Resolver::custom(addr.parse().unwrap(), Some(provider.to_owned()), None, None);
        r.id = id.into();
        r.group = group.into();
        r
    }

    fn unnamed(addr: &str) -> Resolver {
        let mut r = Resolver::custom(addr.parse().unwrap(), None, None, None);
        r.id = format!("c-{addr}");
        r
    }

    fn eg(addr: &str, asn: Option<u32>) -> Egress {
        Egress { addr: addr.parse().unwrap(), asn }
    }

    #[test]
    fn operators_merge_by_provider_catalog_address_and_egress_as() {
        let mut alpha2 = res("alpha-2", "ALPHA", "public", "192.0.2.2");
        alpha2.variant = "Family".into();
        let alpha = res("alpha", "Alpha", "public", "192.0.2.1");
        let beta = res("beta", "Beta", "public", "198.51.100.1");
        let none = HashMap::new();

        // Two entries of one provider, case-insensitively.
        let o = operators(&[alpha.clone(), alpha2, beta.clone()], &[], &none);
        assert_eq!(o.op[0], o.op[1]);
        assert_ne!(o.op[0], o.op[2]);
        assert_eq!(o.independent_count(), 2);

        // Two unnamed customs with unrelated addresses stay two.
        let o = operators(&[unnamed("192.0.2.77"), unnamed("203.0.113.77")], &[], &none);
        assert_ne!(o.op[0], o.op[1]);

        // A custom on a catalog address joins that provider.
        let o = operators(&[beta.clone(), unnamed("198.51.100.1")], std::slice::from_ref(&beta), &none);
        assert_eq!(o.op[0], o.op[1]);
        let o = operators(&[alpha.clone(), unnamed("198.51.100.1")], &[alpha.clone(), beta.clone()], &none);
        assert_ne!(o.op[0], o.op[1]);

        // A forwarder whose egress AS matches a monitored operator merges with it.
        let fwd = res("system-0", "System resolver", "system", "192.168.0.1");
        let egress =
            HashMap::from([("alpha".to_owned(), eg("203.0.113.5", Some(64496))), ("system-0".to_owned(), eg("203.0.113.9", Some(64496)))]);
        let o = operators(&[alpha.clone(), beta.clone(), fwd.clone()], &[], &egress);
        assert_eq!(o.op[0], o.op[2]);
        assert!(o.independent[2]);
        assert_eq!(o.independent_count(), 2);

        // Another egress AS: its own operator.
        let egress =
            HashMap::from([("alpha".to_owned(), eg("203.0.113.5", Some(64496))), ("system-0".to_owned(), eg("198.51.100.9", Some(64497)))]);
        let o = operators(&[alpha.clone(), beta.clone(), fwd.clone()], &[], &egress);
        assert_ne!(o.op[0], o.op[2]);
        assert!(o.independent[2]);
        assert_eq!(o.independent_count(), 3);

        // An unknown AS merges only on equal egress addresses.
        let same = HashMap::from([("alpha".to_owned(), eg("203.0.113.5", None)), ("system-0".to_owned(), eg("203.0.113.5", None))]);
        assert_eq!(operators(&[alpha.clone(), fwd.clone()], &[], &same).independent_count(), 1);
        let other = HashMap::from([("alpha".to_owned(), eg("203.0.113.5", None)), ("system-0".to_owned(), eg("203.0.113.6", None))]);
        assert_eq!(operators(&[alpha.clone(), fwd.clone()], &[], &other).independent_count(), 2);

        // Never looked up: not independent, and the public ones still count.
        let o = operators(&[alpha, beta, fwd], &[], &none);
        assert_eq!(o.independent, [true, true, false]);
        assert_eq!(o.independent_count(), 2);
        assert_eq!(o.count(), 3);
    }

    #[test]
    fn a_failed_cycle_is_isolated_shared_or_unclear() {
        use Cause::*;
        // (operator, independent, answered)
        // One operator's addresses silent, peers fine.
        assert_eq!(attribute(&[(0, true, false), (1, true, true), (2, true, true)]), [Some(Isolated), None, None]);
        // All three fail.
        assert_eq!(attribute(&[(0, true, false), (1, true, false), (2, true, false)]), [Some(Shared); 3]);
        // Two of three.
        assert_eq!(attribute(&[(0, true, false), (1, true, false), (2, true, true)]), [Some(Unclear), Some(Unclear), None]);
        // A forwarder merged with its failing upstream (one operator), the others answer.
        assert_eq!(
            attribute(&[(0, true, false), (0, true, false), (1, true, true), (2, true, true)]),
            [Some(Isolated), Some(Isolated), None, None]
        );
        // That pair failing with nothing else monitored.
        assert_eq!(attribute(&[(0, true, false), (0, true, false)]), [Some(Isolated), Some(Isolated)]);
        // One independent operator monitored.
        assert_eq!(attribute(&[(0, true, false)]), [Some(Isolated)]);
        // A resolver outside the quorum doesn't make a shared verdict, and its own failure is read against the others.
        assert_eq!(attribute(&[(0, true, false), (1, false, false), (2, true, true)]), [Some(Isolated), Some(Isolated), None]);
        // An operator with one answered resolver is answered.
        assert_eq!(attribute(&[(0, true, false), (0, true, true), (1, true, false)]), [Some(Isolated), None, Some(Isolated)]);
        assert!(attribute(&[]).is_empty());
    }
}
