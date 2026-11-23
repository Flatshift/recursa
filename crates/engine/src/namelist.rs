//! Name lists for the Extended test: the bundled lists, the Tranco tail, and the plan that orders a run's lookups.
//!
//! A run asks a tenth of its lookups three times each, from a short list of common names with long TTLs
//! (the resolver should hold them), and fills the rest with names that are rarely asked: a sample of the Tranco
//! list when it can be fetched, the bundled pool when it cannot.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

const COMMON: &str = include_str!("../../../data/names-common.txt");
const POOL: &str = include_str!("../../../data/names-pool.txt");

const TRANCO_ID_URL: &str = "https://tranco-list.eu/top-1m-id";
const TRANCO_ZIP_URL: &str = "https://tranco-list.eu/top-1m.csv.zip";
const TRANCO_KEY: &str = "names:tranco";
const TRANCO_WAIT_KEY: &str = "names:tranco:wait";

const DAY_MS: i64 = 86_400_000;
const MAX_AGE_MS: i64 = 30 * DAY_MS;
const MAX_WAIT_MS: i64 = 7 * DAY_MS;
/// The largest download we accept.
const MAX_ZIP: usize = 32 << 20;
/// The largest list we accept once unpacked.
const MAX_CSV: u64 = 128 << 20;
/// Ranks below this are skipped: the very popular names are what the common list is for.
const MIN_RANK: u32 = 10_001;
const MAX_RANK: u32 = 1_000_000;
/// Fewer valid rows than this means the file isn't the list we expect.
const MIN_ROWS: usize = 100_000;
/// 20 times the sample of a 5,000-lookup run.
const CANDIDATES: usize = 70_000;

/// Names that can't be registered on the public internet.
const BLOCKED_TLDS: &[&str] = &["local", "localhost", "internal", "onion", "test", "invalid", "example", "lan", "home", "corp", "arpa"];
/// Country TLDs left out of every list: filtering resolvers and networks often block or drop names under them, which reads as loss.
const EXCLUDED_TLDS: &[&str] = &["ru", "su", "xn--p1ai", "cn", "xn--fiqs8s", "xn--fiqz9s", "by", "ir", "kp", "sy", "cu"];

fn lines(text: &'static str) -> Vec<&'static str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

/// The common names, in file order.
pub(crate) fn common() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| lines(COMMON))
}

/// The bundled pool of names, in file order (sorted).
pub(crate) fn pool() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| lines(POOL))
}

/// The date the bundled pool was built, from its `# Built:` header line.
fn pool_date() -> String {
    POOL.lines().find_map(|l| l.strip_prefix("# Built:")).map(|d| d.trim().chars().take(10).collect()).unwrap_or_default()
}

fn tld(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

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

/// Where a run's tail names came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct TailSource {
    /// `tranco` (fetched now), `tranco-cached` or `bundled`.
    pub kind: &'static str,
    /// The Tranco list ID, or `pool-` and the build date for the bundled pool.
    pub id: String,
    /// The list's date, or the pool's build date (`YYYY-MM-DD`).
    pub date: String,
    pub names: Vec<String>,
}

/// What a fetch brings back: the list ID and the unpacked `rank,domain` text.
pub(crate) struct Download {
    pub id: String,
    pub csv: String,
}

/// Why a fetch gave nothing.
pub(crate) enum Failure {
    /// The server asked us to wait this many seconds.
    Wait(i64),
    Error,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    id: String,
    date: String,
    fetched_at: i64,
    names: Vec<String>,
}

/// `YYYY-MM-DD` (UTC) for Unix milliseconds.
fn date_of(ms: i64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = ms.div_euclid(DAY_MS) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Splitmix64: small, and the same on every platform, so `(list id, seed)` reproduces a plan.
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

/// A seed from text (FNV-1a), so the candidate set for one list ID is always the same.
fn seed_of(text: &str) -> u64 {
    text.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3))
}

/// The `rank,domain` lines that make usable tail names, reduced to a uniform candidate set. None if the file has
/// too few valid rows to be the list we expect.
fn candidates(csv: &str, id: &str) -> Option<Vec<String>> {
    let common: HashSet<&str> = common().iter().copied().collect();
    let mut names: Vec<String> = csv
        .lines()
        .filter_map(|l| {
            let (rank, domain) = l.split_once(',')?;
            let rank: u32 = rank.trim().parse().ok()?;
            let domain = domain.trim();
            ((MIN_RANK..=MAX_RANK).contains(&rank) && valid(domain) && domain != "tranco-list.eu" && !common.contains(domain))
                .then(|| domain.to_string())
        })
        .collect();
    if names.len() < MIN_ROWS {
        return None;
    }
    Rng(seed_of(id)).shuffle(&mut names);
    names.truncate(CANDIDATES);
    Some(names)
}

fn le16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

/// The text of the one deflate entry in a zip file. Anything else (more entries, other methods, encryption) is None.
fn unzip_single(zip: &[u8]) -> Option<String> {
    if zip.get(..4)? != b"PK\x03\x04" {
        return None;
    }
    let (flags, method) = (le16(zip, 6)?, le16(zip, 8)?);
    if flags & 1 != 0 || method != 8 {
        return None;
    }
    let start = 30 + le16(zip, 26)? + le16(zip, 28)?;
    // The end-of-central-directory record, at most 64 KiB of comment from the end, says how many entries there are.
    let from = zip.len().saturating_sub(22 + 65_535);
    let eocd = zip[from..].windows(4).rposition(|w| w == b"PK\x05\x06")? + from;
    if le16(zip, eocd + 10)? != 1 {
        return None;
    }
    // The header's sizes are zero when a data descriptor follows, and deflate ends itself, so read to its end.
    let mut out = String::new();
    flate2::read::DeflateDecoder::new(zip.get(start..eocd)?).take(MAX_CSV).read_to_string(&mut out).ok()?;
    Some(out)
}

/// The Tranco list ID and top-1m rows, over HTTPS through the system's own DNS.
async fn fetch() -> Result<Download, Failure> {
    let http = reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(60))
        .https_only(true)
        .build()
        .map_err(|_| Failure::Error)?;
    let id = String::from_utf8(get(&http, TRANCO_ID_URL, 256).await?).map_err(|_| Failure::Error)?.trim().to_string();
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Failure::Error);
    }
    let zip = get(&http, TRANCO_ZIP_URL, MAX_ZIP).await?;
    let csv = unzip_single(&zip).ok_or(Failure::Error)?;
    Ok(Download { id, csv })
}

async fn get(http: &reqwest::Client, url: &str, cap: usize) -> Result<Vec<u8>, Failure> {
    let mut r = http.get(url).send().await.map_err(|_| Failure::Error)?;
    let status = r.status();
    let wait = r.headers().get("retry-after").map(|v| v.to_str().ok().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(DAY_MS / 1000));
    if let Some(secs) = wait.or((status.as_u16() == 429).then_some(DAY_MS / 1000)) {
        return Err(Failure::Wait(secs));
    }
    if !status.is_success() || r.content_length().is_some_and(|n| n as usize > cap) {
        return Err(Failure::Error);
    }
    // Read in pieces: a chunked reply has no length to check up front, and must not fill memory first.
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| Failure::Error)? {
        if body.len() + chunk.len() > cap {
            return Err(Failure::Error);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn cached(store: &Store) -> Option<Cached> {
    serde_json::from_value(store.cache_get_any(TRANCO_KEY)?.0).ok()
}

fn bundled() -> TailSource {
    let date = pool_date();
    TailSource { kind: "bundled", id: format!("pool-{date}"), date, names: pool().iter().map(|n| n.to_string()).collect() }
}

fn from_cache(c: Cached) -> TailSource {
    TailSource { kind: "tranco-cached", id: c.id, date: c.date, names: c.names }
}

/// The names a run's tail is drawn from: a cache younger than 30 days, else a fresh download, else a cache of any
/// age, else the bundled pool. Call it at the start of a run, before any timed query.
pub(crate) async fn tail_source(store: &Store, now: i64) -> TailSource {
    tail_source_with(store, now, fetch).await
}

async fn tail_source_with<F, Fut>(store: &Store, now: i64, fetch: F) -> TailSource
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Download, Failure>>,
{
    let old = cached(store);
    if let Some(c) = old.filter(|c| now - c.fetched_at < MAX_AGE_MS) {
        return from_cache(c);
    }
    let waiting = store.cache_get_any(TRANCO_WAIT_KEY).and_then(|(v, _)| v["until"].as_i64()).is_some_and(|until| until > now);
    if !waiting {
        match fetch().await {
            Ok(d) => {
                if let Some(names) = candidates(&d.csv, &d.id) {
                    let date = date_of(now);
                    store.cache_put(TRANCO_KEY, &json!({ "id": d.id, "date": date, "fetched_at": now, "names": names }));
                    return TailSource { kind: "tranco", id: d.id, date, names };
                }
            }
            Err(Failure::Wait(secs)) => {
                let until = now + (secs.max(0) * 1000).min(MAX_WAIT_MS);
                store.cache_put(TRANCO_WAIT_KEY, &json!({ "until": until }));
            }
            Err(Failure::Error) => {}
        }
    }
    cached(store).map(from_cache).unwrap_or_else(bundled)
}

/// A run's lookups in the order they are asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plan {
    /// `total` names in asked order.
    pub names: Vec<String>,
    /// 0 for a tail name; 1 to 3 for the first, second or third ask of a common name.
    pub touch: Vec<u8>,
}

/// A tenth of `total` common names, each asked once in each third of the run at the same position in the third, and
/// tail names from `source` in every other slot, with no TLD above a quarter of the tail. The same list and seed
/// give the same plan.
pub(crate) fn plan(total: usize, source: &TailSource, seed: u64) -> Plan {
    let mut rng = Rng(seed);
    let pick = (total / 10).min(common().len());
    let mut chosen: Vec<&str> = common().to_vec();
    rng.shuffle(&mut chosen);
    chosen.truncate(pick);

    // The common names' places in a third; the last third also holds the remainder, all tail.
    let third = total / 3;
    let mut slots: Vec<usize> = (0..third).collect();
    rng.shuffle(&mut slots);
    slots.truncate(pick);
    let at: HashMap<usize, &str> = slots.into_iter().zip(chosen).collect();

    let tail_n = total - 3 * pick;
    let mut tail = tail_names(&source.names, tail_n, &mut rng).into_iter();
    let mut names = Vec::with_capacity(total);
    let mut touch = Vec::with_capacity(total);
    for pos in 0..total {
        let (in_third, ask) = (pos % third.max(1), (pos / third.max(1)).min(2) as u8 + 1);
        match at.get(&in_third).filter(|_| pos < 3 * third) {
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

/// `n` names from `source`, shuffled, with no TLD above a quarter of `n`. A source too small or too lopsided for
/// that is topped up from what the cap left out.
fn tail_names(source: &[String], n: usize, rng: &mut Rng) -> Vec<String> {
    let mut all: Vec<&String> = source.iter().collect();
    rng.shuffle(&mut all);
    let cap = (n / 4).max(1);
    let mut per_tld: HashMap<&str, usize> = HashMap::new();
    let (mut out, mut left) = (Vec::with_capacity(n), Vec::new());
    for name in all {
        let seen = per_tld.entry(tld(name)).or_default();
        if out.len() < n && *seen < cap {
            *seen += 1;
            out.push(name.clone());
        } else {
            left.push(name.clone());
        }
    }
    out.extend(left.into_iter().take(n - out.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::{remove, temp_path};
    use std::io::Write;

    fn store() -> (Store, std::path::PathBuf) {
        let path = temp_path();
        (Store::open(path.clone()).unwrap(), path)
    }

    fn source(n: usize) -> TailSource {
        // Four TLDs, so a quarter cap is reachable but only just.
        let names = (0..n).map(|i| format!("site{i}.{}", ["com", "net", "org", "info", "de", "fr"][i % 6])).collect();
        TailSource { kind: "bundled", id: "t".into(), date: "2026-01-01".into(), names }
    }

    #[test]
    fn bundled_lists_are_read() {
        assert!(common().iter().all(|n| !n.starts_with('#') && n.trim() == *n));
        assert!(!pool().is_empty());
        for n in common().iter().chain(pool()) {
            assert!(valid(n), "{n}");
        }
        assert_eq!(pool_date().len(), 10);
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
            "example.com.cn",
            "example.xn--p1ai",
            "example.ir",
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

    fn zip_of(csv: &str, entries: u16, method: u16) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(csv.as_bytes()).unwrap();
        let data = enc.finish().unwrap();
        let mut z = b"PK\x03\x04".to_vec();
        z.extend_from_slice(&[20, 0, 0, 0]);
        z.extend_from_slice(&method.to_le_bytes());
        z.extend_from_slice(&[0; 8]);
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&(csv.len() as u32).to_le_bytes());
        z.extend_from_slice(&[7, 0, 0, 0]);
        z.extend_from_slice(b"top.csv");
        z.extend_from_slice(&data);
        z.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        z.extend_from_slice(&entries.to_le_bytes());
        z.extend_from_slice(&entries.to_le_bytes());
        z.extend_from_slice(&[0; 10]);
        z
    }

    #[test]
    fn zip_with_one_deflate_entry_unpacks() {
        let csv = "1,example.com\n2,example.net\n";
        assert_eq!(unzip_single(&zip_of(csv, 1, 8)).as_deref(), Some(csv));
    }

    #[test]
    fn other_zips_are_refused() {
        let csv = "1,example.com\n";
        assert!(unzip_single(&zip_of(csv, 2, 8)).is_none(), "two entries");
        assert!(unzip_single(&zip_of(csv, 1, 0)).is_none(), "stored, not deflated");
        assert!(unzip_single(b"<html>not a zip</html>").is_none());
        assert!(unzip_single(&zip_of(csv, 1, 8)[..20]).is_none(), "cut short");
        assert!(unzip_single(b"").is_none());
    }

    /// Every ask of every common name, and where the names fall.
    #[test]
    fn plan_invariants() {
        let src = source(20_000);
        let total = 5_000;
        let p = plan(total, &src, 7);
        assert_eq!((p.names.len(), p.touch.len()), (total, total));
        let third = total / 3;
        let mut asks: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, (n, t)) in p.names.iter().zip(&p.touch).enumerate() {
            if *t > 0 {
                assert_eq!(usize::from(*t), i / third + 1, "ask number follows the third");
                asks.entry(n).or_default().push(i);
            }
        }
        assert_eq!(asks.len(), total / 10);
        for (name, at) in &asks {
            assert!(common().contains(name));
            assert_eq!(at.len(), 3, "{name}");
            assert_eq!((at[1] - at[0], at[2] - at[1]), (third, third), "{name}");
        }
        let tail: Vec<&String> = p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0).map(|(n, _)| n).collect();
        assert_eq!(tail.len(), total - 3 * (total / 10));
        assert_eq!(tail.iter().collect::<HashSet<_>>().len(), tail.len(), "no tail name twice");
        let mut per_tld: HashMap<&str, usize> = HashMap::new();
        for n in &tail {
            *per_tld.entry(tld(n)).or_default() += 1;
        }
        assert!(per_tld.values().all(|c| *c * 4 <= tail.len()), "{per_tld:?}");
    }

    #[test]
    fn plan_follows_the_seed() {
        let src = source(20_000);
        let (a, b, c) = (plan(900, &src, 1), plan(900, &src, 1), plan(900, &src, 2));
        assert_eq!(a, b);
        let tail = |p: &Plan| p.names.iter().zip(&p.touch).filter(|(_, t)| **t == 0).map(|(n, _)| n.clone()).collect::<Vec<_>>();
        assert_ne!(tail(&a), tail(&c));
    }

    #[test]
    fn plan_for_a_total_that_is_not_a_multiple_of_three() {
        let p = plan(1_001, &source(5_000), 3);
        assert_eq!(p.names.len(), 1_001);
        assert_eq!(p.touch.iter().filter(|t| **t > 0).count(), 300);
    }

    fn download(id: &str, rows: usize) -> Download {
        let csv = (1..=rows).map(|i| format!("{i},site{i}.com\n")).collect();
        Download { id: id.into(), csv }
    }

    async fn no_fetch() -> Result<Download, Failure> {
        panic!("fetched when it should not have");
    }

    #[tokio::test]
    async fn fetched_list_is_cached_and_then_used_without_a_request() {
        let (store, path) = store();
        let now = 1_800_000_000_000;
        let first = tail_source_with(&store, now, || async { Ok(download("AB12C", 120_000)) }).await;
        assert_eq!((first.kind, first.id.as_str(), first.names.len()), ("tranco", "AB12C", CANDIDATES));
        assert_eq!(first.date, date_of(now));
        assert!(first.names.iter().all(|n| valid(n)));
        let again = tail_source_with(&store, now + 29 * DAY_MS, no_fetch).await;
        assert_eq!((again.kind, again.names), ("tranco-cached", first.names));
        remove(&path);
    }

    #[tokio::test]
    async fn fallback_order() {
        let (store, path) = store();
        let now = 1_800_000_000_000;
        // Nothing cached, no list: the bundled pool.
        let s = tail_source_with(&store, now, || async { Err(Failure::Error) }).await;
        assert_eq!((s.kind, s.names.len()), ("bundled", pool().len()));
        // A list with too few rows is a format failure.
        let s = tail_source_with(&store, now, || async { Ok(download("SHORT", 2_000)) }).await;
        assert_eq!(s.kind, "bundled");
        // A fetch stores a list; once it is old and the next fetch fails, the old list answers.
        tail_source_with(&store, now, || async { Ok(download("OLD01", 120_000)) }).await;
        let s = tail_source_with(&store, now + 31 * DAY_MS, || async { Err(Failure::Error) }).await;
        assert_eq!((s.kind, s.id.as_str()), ("tranco-cached", "OLD01"));
        // Past 30 days a new fetch replaces it.
        let s = tail_source_with(&store, now + 31 * DAY_MS, || async { Ok(download("NEW02", 120_000)) }).await;
        assert_eq!((s.kind, s.id.as_str()), ("tranco", "NEW02"));
        remove(&path);
    }

    #[tokio::test]
    async fn a_wait_request_is_stored_and_respected() {
        let (store, path) = store();
        let now = 1_800_000_000_000;
        // A month asked for is stored as the 7-day cap.
        let s = tail_source_with(&store, now, || async { Err(Failure::Wait(30 * 86_400)) }).await;
        assert_eq!(s.kind, "bundled");
        let until = store.cache_get_any(TRANCO_WAIT_KEY).unwrap().0["until"].as_i64().unwrap();
        assert_eq!(until, now + MAX_WAIT_MS);
        // No request while the wait lasts, one after.
        let s = tail_source_with(&store, now + DAY_MS, no_fetch).await;
        assert_eq!(s.kind, "bundled");
        let s = tail_source_with(&store, now + 8 * DAY_MS, || async { Ok(download("AFTER", 120_000)) }).await;
        assert_eq!(s.kind, "tranco");
        remove(&path);
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(1_800_000_000_000), "2027-01-15");
        assert_eq!(date_of(951_782_400_000), "2000-02-29");
    }
}
