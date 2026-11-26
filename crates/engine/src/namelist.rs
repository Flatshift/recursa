//! Name lists for the Extended test: the bundled lists and the plan that orders a run's lookups.
//!
//! A run asks a tenth of its lookups eight times each, from a short list of common names with long TTLs
//! (the resolver should hold them), and fills the rest with names that are rarely asked, sampled from the bundled
//! pool: names from the user's part of the world first, when the country is known. The pool is screened when it is
//! built; nothing is downloaded at run time.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const COMMON: &str = include_str!("../../../data/names-common.txt");
const POOL: &str = include_str!("../../../data/names-pool.txt");

fn lines(text: &'static str) -> Vec<&'static str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

/// The common names, in file order.
pub(crate) fn common() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| lines(COMMON))
}

/// The bundled pool in file order (sorted): each name with its country as an ISO 3166 two-letter code, or `""`
/// where the pool records none.
pub(crate) fn pool() -> &'static [(&'static str, &'static str)] {
    static LIST: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    LIST.get_or_init(|| lines(POOL).into_iter().map(|l| l.split_once(' ').unwrap_or((l, ""))).collect())
}

/// The five regional internet registries and the countries each serves, from the NRO's list of country codes by
/// registry. A code newer than that list takes the registry of the country it came from.
#[rustfmt::skip]
const REGISTRIES: [(&str, &str); 5] = [
    ("afrinic", "AO BF BI BJ BW CD CF CG CI CM CV DJ DZ EG EH ER ET GA GH GM GN GQ GW KE KM LR LS LY MA MG ML MR MU MW MZ NA NE NG RE RW SC SD SL SN SO SS ST SZ TD TG TN TZ UG YT ZA ZM ZW"),
    ("apnic", "AF AS AU BD BN BT CC CK CN CX FJ FM GU HK ID IN IO JP KH KI KP KR LA LK MH MM MN MO MP MV MY NC NF NP NR NU NZ PF PG PH PK PN PW SB SG TF TH TK TL TO TV TW VN VU WF WS"),
    ("arin", "AG AI AQ BB BL BM BS BV CA DM GD GP HM JM KN KY LC MF MQ MS PM PR SH TC UM US VC VG VI"),
    ("lacnic", "AR AW BO BQ BR BZ CL CO CR CU CW DO EC FK GF GS GT GY HN HT MX NI PA PE PY SR SV SX TT UY VE"),
    ("ripe", "AD AE AL AM AT AX AZ BA BE BG BH BY CH CY CZ DE DK EE ES FI FO FR GB GE GG GI GL GR HR HU IE IL IM IQ IR IS IT JE JO KG KW KZ LB LI LT LU LV MC MD ME MK MT NL NO OM PL PS PT QA RO RS RU SA SE SI SJ SK SM SY TJ TM TR UA UZ VA XK YE"),
];

fn serves(codes: &str, country: &str) -> bool {
    codes.split(' ').any(|c| c == country)
}

/// The date the bundled pool was built, from its `# Built:` header line.
fn pool_date() -> String {
    POOL.lines().find_map(|l| l.strip_prefix("# Built:")).map(|d| d.trim().chars().take(10).collect()).unwrap_or_default()
}

fn tld(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// A tail name's TLD class: 0 for `.com`, 1 for another generic TLD, 2 for a country domain (two letters).
pub(crate) fn class(name: &str) -> usize {
    match tld(name) {
        "com" => 0,
        t if t.len() == 2 => 2,
        _ => 1,
    }
}

/// Where a run's tail names came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct TailSource {
    /// Always `bundled`.
    pub kind: &'static str,
    /// `pool-` and the build date of the bundled pool.
    pub id: String,
    /// The pool's build date (`YYYY-MM-DD`).
    pub date: String,
    /// The registry (`arin`, `ripe`, …) that serves the user's country, when the country is known.
    pub region: Option<&'static str>,
    /// The pool's names from that registry's countries.
    pub regional: Vec<&'static str>,
    /// Every other name.
    pub names: Vec<&'static str>,
}

/// Splitmix64: small, and the same on every platform, so `(list, seed)` reproduces a plan.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

/// The bundled pool, split around the registry that serves `country` (an ISO 3166 two-letter code).
pub(crate) fn bundled(country: Option<&str>) -> TailSource {
    let date = pool_date();
    let registry = country.and_then(|c| REGISTRIES.iter().find(|r| serves(r.1, c)));
    let (mut regional, mut names) = (Vec::new(), Vec::new());
    for &(name, cc) in pool() {
        if registry.is_some_and(|r| serves(r.1, cc)) { &mut regional } else { &mut names }.push(name);
    }
    TailSource { kind: "bundled", id: format!("pool-{date}"), date, region: registry.map(|r| r.0), regional, names }
}

/// A run's lookups in the order they are asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    /// `total` names in asked order.
    pub names: Vec<String>,
    /// 0 for a tail name; 1 to the plan's asks per common name for the nth ask of one.
    pub touch: Vec<u8>,
}

/// How many times a common name is asked across a run, unless the list is too short to fill four fifths of it.
const ASKS: usize = 8;

/// How many times each of `pick` common names is asked to fill four fifths of `total`: `ASKS`, or more when the list
/// ran out before a tenth of `total`.
fn asks_for(total: usize, pick: usize) -> usize {
    if pick < total / 10 { (total / 10 * ASKS).div_ceil(pick.max(1)).min(u8::MAX as usize) } else { ASKS }
}

/// A tenth of `total` common names (as many as the list has), each asked once in each part of the run at the same
/// position in the part, and tail names from `source` in every other slot. Eight asks and parts, more when the list is
/// too short, so that four fifths of the lookups stay common. The same list, country and seed give the same plan.
pub(crate) fn plan(total: usize, source: &TailSource, seed: u64) -> Plan {
    let mut rng = Rng(seed);
    let pick = (total / 10).min(common().len());
    let mut chosen: Vec<&str> = common().to_vec();
    rng.shuffle(&mut chosen);
    chosen.truncate(pick);

    // The common names' places in a part; the end of the run also holds the remainder, all tail.
    let asks = asks_for(total, pick);
    let part = total / asks;
    let mut slots: Vec<usize> = (0..part).collect();
    rng.shuffle(&mut slots);
    slots.truncate(pick);
    let at: HashMap<usize, &str> = slots.into_iter().zip(chosen).collect();

    let tail_n = total - asks * pick;
    let mut tail = tail_names(source, tail_n, &mut rng).into_iter();
    let mut names = Vec::with_capacity(total);
    let mut touch = Vec::with_capacity(total);
    for pos in 0..total {
        let (in_part, ask) = (pos % part.max(1), (pos / part.max(1)).min(asks - 1) as u8 + 1);
        match at.get(&in_part).filter(|_| pos < asks * part) {
            Some(name) => {
                names.push(name.to_string());
                touch.push(ask);
            }
            None => {
                let Some(name) = tail.next() else { break };
                names.push(name);
                touch.push(0);
            }
        }
    }
    Plan { names, touch }
}

/// `n` names from `source`, shuffled. The regional names come first, each as likely as the next. What they leave is
/// filled from the other names: a quarter from generic TLDs other than `.com`, 15 in 100 from country domains and the
/// rest from `.com`, because those names are mostly country domains. A class that runs short is topped up from what
/// the mix left out.
fn tail_names(source: &TailSource, n: usize, rng: &mut Rng) -> Vec<String> {
    let mut out = source.regional.clone();
    rng.shuffle(&mut out);
    out.truncate(n);
    let fill = n - out.len();
    let mut all = source.names.clone();
    rng.shuffle(&mut all);
    let (generic, country) = (fill / 4, fill * 15 / 100);
    let mut room = [fill - generic - country, generic, country];
    let mut left = Vec::new();
    for name in all {
        let room = &mut room[class(name)];
        if *room > 0 {
            *room -= 1;
            out.push(name);
        } else {
            left.push(name);
        }
    }
    out.extend(left.into_iter().take(n - out.len()));
    // The classes fill at different rates, so the order taken is not the order asked.
    rng.shuffle(&mut out);
    out.into_iter().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Names that can't be registered on the public internet.
    const BLOCKED_TLDS: &[&str] = &["local", "localhost", "internal", "onion", "test", "invalid", "example", "lan", "home", "corp", "arpa"];
    /// TLDs left out of every list: filtering resolvers, firewalls and intrusion prevention often block or drop names under
    /// them, which reads as loss. Country TLDs first, then TLDs that common intrusion rules or abuse rankings single out, then
    /// the adult TLDs, which no list should carry.
    #[rustfmt::skip]
    const EXCLUDED_TLDS: &[&str] = &[
        "ru", "su", "xn--p1ai", "cn", "xn--fiqs8s", "xn--fiqz9s", "by", "ir", "kp", "sy", "cu",
        "top", "to", "cc", "tk", "pw", "gq", "ga", "ml", "cf", "gdn", "icu", "biz", "okinawa", "desi", "life", "work", "world",
        "fit", "ryukyu", "cloud", "foo", "zip", "dad", "prof", "mov", "nexus", "phd", "esq", "win", "cfd", "management", "fyi",
        "beauty", "xin", "bond", "help", "cyou", "sbs", "quest", "rest", "vip", "lol", "cam", "surf", "monster", "buzz", "date",
        "xyz", "click", "shop",
        "xxx", "porn", "adult", "sex", "sexy", "webcam",
    ];

    /// Whether `name` is a lower-case registrable domain under an ICANN suffix, written with letters, digits and hyphens.
    pub(crate) fn valid(name: &str) -> bool {
        if name.len() > 253 || name.split('.').count() < 2 {
            return false;
        }
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if !name.split('.').all(label_ok) {
            return false;
        }
        let last = tld(name);
        // An all-digit last label is an address, not a name.
        if last.bytes().all(|b| b.is_ascii_digit()) || BLOCKED_TLDS.contains(&last) || EXCLUDED_TLDS.contains(&last) {
            return false;
        }
        let Some(domain) = psl::domain(name.as_bytes()) else {
            return false;
        };
        domain.as_bytes() == name.as_bytes() && domain.suffix().typ() == Some(psl::Type::Icann)
    }

    fn source(n: usize) -> TailSource {
        // Of every six names, one is `.com`, three are other generic TLDs and two are country domains.
        let names = (0..n).map(|i| &*format!("site{i}.{}", ["com", "net", "org", "info", "de", "fr"][i % 6]).leak()).collect();
        TailSource { kind: "bundled", id: "t".into(), date: "2026-01-01".into(), region: None, regional: Vec::new(), names }
    }

    /// `source(n)` with `regional` names of a region set apart: `local0.de`, `local1.com` and so on.
    fn regional_source(n: usize, regional: usize) -> TailSource {
        let names = (0..regional).map(|i| &*format!("local{i}.{}", ["de", "com"][i % 2]).leak()).collect();
        TailSource { region: Some("ripe"), regional: names, ..source(n) }
    }

    #[test]
    fn bundled_lists_are_read() {
        assert!(common().iter().all(|n| !n.starts_with('#') && n.trim() == *n));
        assert!(!pool().is_empty());
        for n in common().iter().copied().chain(pool().iter().map(|p| p.0)) {
            assert!(valid(n), "{n}");
        }
        for (n, cc) in pool() {
            assert!(cc.is_empty() || (cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase())), "{n} {cc}");
        }
        assert_eq!(pool_date().len(), 10);
    }

    #[test]
    fn every_country_has_one_registry() {
        let codes: Vec<&str> = REGISTRIES.iter().flat_map(|r| r.1.split(' ')).collect();
        assert!(codes.iter().all(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())));
        assert_eq!(codes.iter().collect::<HashSet<_>>().len(), codes.len());
    }

    #[test]
    fn bundled_pool_splits_by_registry() {
        for (country, key) in [("US", "arin"), ("DE", "ripe"), ("JP", "apnic"), ("BR", "lacnic"), ("ZA", "afrinic")] {
            let src = bundled(Some(country));
            assert_eq!(src.region, Some(key));
            assert!(src.regional.len() >= 1_000, "{key} fills a run's once-asked names");
            assert_eq!(src.regional.len() + src.names.len(), pool().len());
        }
        // No country, or one no registry serves: the whole pool, as before.
        for country in [None, Some("XX"), Some("T1"), Some("")] {
            let src = bundled(country);
            assert_eq!((src.region, src.regional.len(), src.names.len()), (None, 0, pool().len()), "{country:?}");
        }
    }

    #[test]
    fn regional_names_are_asked_first() {
        let regional = |p: &Plan| p.names.iter().filter(|n| n.starts_with("local")).count();
        // 1,000 tail names: a region with fewer gives all it has, and the other names fill the rest by the mix.
        let p = plan(5_000, &regional_source(20_000, 300), 7);
        assert_eq!((p.names.len(), regional(&p)), (5_000, 300));
        let mut rest = [0; 3];
        for (n, _) in p.names.iter().zip(&p.touch).filter(|(n, t)| **t == 0 && !n.starts_with("local")) {
            rest[class(n)] += 1;
        }
        assert_eq!(rest, [420, 175, 105]);
        // A region with enough names fills every tail slot.
        let p = plan(5_000, &regional_source(20_000, 9_000), 7);
        assert_eq!(regional(&p), 1_000);
        assert_eq!(p.names.iter().collect::<HashSet<_>>().len(), 1_500);
    }

    #[test]
    fn validation_table() {
        for good in ["example.com", "a-b.org", "xn--bcher-kva.de", "example.co.uk", "1example.net", "a1.io"] {
            assert!(valid(good), "{good}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        let long_name = format!("{}.com", ["abcdefghi"; 30].join("."));
        for bad in [
            "",
            "example",
            "www.example.com",
            "Example.com",
            "exa_mple.com",
            "-example.com",
            "example-.com",
            "example..com",
            "example.com.",
            "bücher.de",
            "192.0.2.1",
            "198.51.100.7",
            "example.local",
            "host.localhost",
            "nas.lan",
            "router.home",
            "x.internal",
            "abc.onion",
            "foo.test",
            "example.ru",
            "example.xxx",
            "example.com.cn",
            "example.xn--p1ai",
            "example.ir",
            "example.top",
            "example.shop",
            "foo.invalid",
            "foo.example",
            "foo.corp",
            "1.0.0.127.in-addr.arpa",
            "co.uk",
            "com",
            "example.blogspot.com",
            "mysite.github.io",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }

    /// Every ask of every common name, and where the names fall.
    #[test]
    fn plan_invariants() {
        let src = source(20_000);
        let total = 5_000;
        let p = plan(total, &src, 7);
        assert_eq!((p.names.len(), p.touch.len()), (total, total));
        let part = total / ASKS;
        let mut asks: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, (n, t)) in p.names.iter().zip(&p.touch).enumerate() {
            if *t > 0 {
                assert_eq!(usize::from(*t), i / part + 1, "ask number follows the eighth");
                asks.entry(n).or_default().push(i);
            }
        }
        assert_eq!(asks.len(), total / 10);
        for (name, at) in &asks {
            assert!(common().contains(name));
            assert_eq!(at.len(), ASKS, "{name}");
            assert!(at.windows(2).all(|w| w[1] - w[0] == part), "{name}");
        }
        // Each common name's asks are touch 1 to 8 in order, and 4,000 of 5,000 lookups are common.
        assert_eq!(p.touch.iter().filter(|t| **t > 0).count(), 4_000);
        for name in asks.keys() {
            let touches: Vec<u8> = p.names.iter().zip(&p.touch).filter(|(n, _)| n == name).map(|(_, t)| *t).collect();
            assert_eq!(touches, (1..=8).collect::<Vec<u8>>(), "{name}");
        }
        let tail: Vec<&String> = p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0).map(|(n, _)| n).collect();
        assert_eq!(tail.len(), 1_000);
        assert_eq!(tail.iter().collect::<HashSet<_>>().len(), tail.len(), "no tail name twice");
        assert_eq!(classes(&p), [600, 250, 150]);
        // The mix holds through the run, not only over the whole of it.
        let early = tail[..tail.len() / 2].iter().filter(|n| class(n) == 0).count();
        assert!((260..=340).contains(&early), "{early}");
    }

    /// How many tail names of each class a plan asks.
    fn classes(p: &Plan) -> [usize; 3] {
        let mut per_class = [0; 3];
        for (n, _) in p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0) {
            per_class[class(n)] += 1;
        }
        per_class
    }

    #[test]
    fn bundled_pool_fills_the_mix() {
        assert_eq!(classes(&plan(5_000, &bundled(None), 7)), [600, 250, 150]);
    }

    #[test]
    fn a_source_short_of_a_class_is_topped_up() {
        // 250 `.com` names where the mix wants 600.
        let p = plan(5_000, &source(1_500), 3);
        assert_eq!(p.names.len(), 5_000);
        assert_eq!(classes(&p)[0], 250);
    }

    #[test]
    fn plan_follows_the_seed() {
        let src = source(20_000);
        let (a, b, c) = (plan(900, &src, 1), plan(900, &src, 1), plan(900, &src, 2));
        assert_eq!(a, b);
        let tail = |p: &Plan| p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0).map(|(n, _)| n.clone()).collect::<Vec<_>>();
        assert_ne!(tail(&a), tail(&c));
    }

    /// 10,000 lookups: the list holds fewer than a tenth of them, so each common name is asked more often.
    #[test]
    fn a_long_plan_keeps_the_mix() {
        let total = 10_000;
        let p = plan(total, &source(20_000), 7);
        assert_eq!(p.names.len(), total);
        let asks = asks_for(total, common().len());
        assert_eq!(asks, 16);
        let mut per_name: HashMap<&str, usize> = HashMap::new();
        for (n, _) in p.names.iter().zip(&p.touch).filter(|(_, t)| **t > 0) {
            *per_name.entry(n).or_default() += 1;
        }
        assert_eq!(per_name.len(), common().len());
        assert!(per_name.values().all(|&c| c == asks));
        let common_n = p.touch.iter().filter(|t| **t > 0).count();
        assert!((7_800..=8_400).contains(&common_n), "{common_n}");
        assert_eq!(*p.touch.iter().max().unwrap() as usize, asks);
        let tail: Vec<&String> = p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0).map(|(n, _)| n).collect();
        assert_eq!(tail.iter().collect::<HashSet<_>>().len(), tail.len(), "no tail name twice");
        assert_eq!(total - common_n, tail.len());
    }

    #[test]
    fn plan_for_a_total_that_is_not_a_multiple_of_eight() {
        let p = plan(1_001, &source(5_000), 3);
        assert_eq!(p.names.len(), 1_001);
        assert_eq!(p.touch.iter().filter(|t| **t > 0).count(), 800);
        assert_eq!(p.touch[1_000], 0, "the remainder is tail");
    }
}
