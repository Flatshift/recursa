//! Who a block of addresses is registered to, from the regional registry's RDAP record.
//!
//! Only addresses that no network announces are asked about, and only past your ISP: the request carries the
//! router's address, never one of yours. Each answer covers a whole block and is kept, so a block is asked about
//! once a month at most. `rdap.org` only redirects to the registry that holds the block (ARIN, RIPE, APNIC,
//! LACNIC or AFRINIC); the record itself comes from there.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The setting that says the user allowed the lookups.
pub(crate) const ACCEPTED: &str = "netmap.rdap_accepted";
const BASE: &str = "https://rdap.org/ip/";
const AS_BASE: &str = "https://rdap.org/autnum/";
/// How long a kept record stands before the block is asked about again.
const MAX_AGE_MS: i64 = 30 * 86_400_000;
/// Between requests, and the most in one map: the registries rate-limit anonymous clients.
const PAUSE: Duration = Duration::from_secs(1);
const PER_RUN: usize = 20;
const TIMEOUT: Duration = Duration::from_secs(20);

/// One registered block, as kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub start: IpAddr,
    pub end: IpAddr,
    pub handle: String,
    /// The block's own name (`FB-BLOCK`, `EQUINIX-IX-DC`).
    pub name: String,
    /// The registrant organisation, when the record names one.
    pub org: Option<String>,
    /// `ARIN`, `RIPE`, `APNIC`, `LACNIC`, `AFRINIC`, or `registry` when the record doesn't say.
    pub rir: String,
    /// When the registry's entry was last changed (ISO date-time), or the registration date.
    pub updated: String,
    pub fetched_at: i64,
}

impl Block {
    /// Who the map names the block after: the organisation, else the block's name.
    pub fn holder(&self) -> &str {
        self.org.as_deref().unwrap_or(&self.name)
    }

    /// The source as a claim cites it.
    pub fn source(&self) -> &'static str {
        match self.rir.as_str() {
            "ARIN" => "ARIN registry",
            "RIPE" => "RIPE registry",
            "APNIC" => "APNIC registry",
            "LACNIC" => "LACNIC registry",
            "AFRINIC" => "AFRINIC registry",
            _ => "registry",
        }
    }
}

/// A sort key for an address: its IPv6 bytes, with IPv4 mapped.
fn key(ip: IpAddr) -> [u8; 16] {
    match ip.to_canonical() {
        IpAddr::V4(a) => a.to_ipv6_mapped().octets(),
        IpAddr::V6(a) => a.octets(),
    }
}

/// Read an RDAP `ip` record. `fetched_at` is now, in ms.
pub fn parse(body: &[u8], fetched_at: i64) -> Result<Block, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let addr = |k: &str| text(k).and_then(|s| s.parse::<IpAddr>().ok()).ok_or_else(|| format!("no {k}"));
    let (start, end) = (addr("startAddress")?, addr("endAddress")?);
    if key(end) < key(start) {
        return Err("the block ends before it starts".into());
    }
    let handle = text("handle").unwrap_or_default();
    let name = text("name").unwrap_or_else(|| handle.clone());
    let events = v.get("events").and_then(|e| e.as_array());
    let event = |action: &str| {
        events?
            .iter()
            .find(|e| e.get("eventAction").and_then(|a| a.as_str()) == Some(action))
            .and_then(|e| e.get("eventDate"))
            .and_then(|d| d.as_str())
            .map(str::to_string)
    };
    let updated = event("last changed").or_else(|| event("registration")).unwrap_or_else(|| super::say::date_of_ms(fetched_at));
    let port43 = text("port43").unwrap_or_default().to_ascii_lowercase();
    let rir = ["arin", "ripe", "apnic", "lacnic", "afrinic"]
        .into_iter()
        .find(|r| port43.contains(r))
        .map(|r| r.to_ascii_uppercase())
        .unwrap_or_else(|| "registry".into());
    let mut found: Vec<(String, String)> = Vec::new();
    registrants(v.get("entities"), &mut found);
    // The organisation only, never a person or a maintainer handle: RIPE lists maintainers as registrants too.
    let org = found.into_iter().find(|(kind, _)| kind == "org").map(|(_, fn_)| fn_);
    Ok(Block { start, end, handle, name, org, rir, updated, fetched_at })
}

/// Who a network's number is registered to: the registrant organisation in its RDAP `autnum` record, if it names one.
/// The request carries the AS number only.
pub fn parse_owner(body: &[u8]) -> Result<Option<String>, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let mut found: Vec<(String, String)> = Vec::new();
    registrants(v.get("entities"), &mut found);
    Ok(found.into_iter().find(|(kind, _)| kind == "org").map(|(_, fn_)| fn_))
}

/// The registered owner of each of `asns`, from what is kept, and when the user allowed it, from the registry for the
/// rest. Paced like [`lookup`]; an answer that names nobody is kept too, so the number is not asked about again.
pub async fn owners<F, Fut>(store: &Store, asns: &[u32], cancel: &AtomicBool, fetch: F) -> HashMap<u32, String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let allowed = store.get::<bool>(ACCEPTED) == Some(true);
    let mut out = HashMap::new();
    let mut asked = 0;
    for &asn in asns {
        let key = format!("asowner:{asn}");
        let keep = |out: &mut HashMap<u32, String>, v: Option<serde_json::Value>| {
            if let Some(o) = v.as_ref().and_then(|v| v.as_str()) {
                out.insert(asn, o.to_string());
            }
        };
        if let Some((v, _)) = store.cache_get(&key, MAX_AGE_MS) {
            keep(&mut out, Some(v));
            continue;
        }
        if !allowed || asked >= PER_RUN || cancel.load(Ordering::SeqCst) {
            keep(&mut out, store.cache_get_any(&key).map(|(v, _)| v));
            continue;
        }
        if asked > 0 {
            tokio::time::sleep(PAUSE).await;
        }
        asked += 1;
        let got = match tokio::time::timeout(TIMEOUT, fetch(format!("{AS_BASE}{asn}"))).await {
            Ok(Ok(body)) => parse_owner(&body),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("timed out".into()),
        };
        match got {
            Ok(org) => {
                store.cache_put(&key, &serde_json::json!(org));
                keep(&mut out, Some(serde_json::json!(org)));
            }
            Err(e) => {
                eprintln!("Registry lookup for AS{asn} failed: {e}");
                keep(&mut out, store.cache_get_any(&key).map(|(v, _)| v));
            }
        }
    }
    out
}

/// Every registrant entity's vCard `kind` and `fn`, at any depth.
fn registrants(entities: Option<&serde_json::Value>, out: &mut Vec<(String, String)>) {
    let Some(list) = entities.and_then(|e| e.as_array()) else { return };
    for e in list {
        let roles = e.get("roles").and_then(|r| r.as_array());
        if roles.is_some_and(|r| r.iter().any(|x| x.as_str() == Some("registrant"))) {
            let items = e.get("vcardArray").and_then(|v| v.get(1)).and_then(|v| v.as_array());
            let field = |name: &str| {
                items?.iter().find(|i| i.get(0).and_then(|n| n.as_str()) == Some(name)).and_then(|i| i.get(3)).and_then(|x| x.as_str())
            };
            if let Some(fn_) = field("fn") {
                out.push((field("kind").unwrap_or("").to_string(), fn_.to_string()));
            }
        }
        registrants(e.get("entities"), out);
    }
}

/// The kept block that holds `ip`, with when it was fetched: the narrowest one when several do.
pub fn cached(store: &Store, ip: IpAddr) -> Option<Block> {
    let k = key(ip).to_vec();
    store.with_conn(|c| {
        c.query_row("SELECT body FROM rdap_block WHERE start <= ?1 AND end >= ?1 ORDER BY start DESC LIMIT 1", rusqlite::params![k], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
    })
}

fn put(store: &Store, b: &Block) {
    let (s, e) = (key(b.start).to_vec(), key(b.end).to_vec());
    let body = serde_json::to_string(b).unwrap_or_default();
    store.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO rdap_block(start, end, body, fetched_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(start) DO UPDATE SET end=excluded.end, body=excluded.body, fetched_at=excluded.fetched_at",
            rusqlite::params![s, e, body, b.fetched_at],
        );
    });
}

/// How many blocks are kept.
pub fn count(store: &Store) -> usize {
    store.with_conn(|c| c.query_row("SELECT COUNT(*) FROM rdap_block", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize)
}

/// The block for each of `addrs`: from what is kept, and when the user allowed it, from the registry for the rest.
/// One request per block, [`PAUSE`] apart, at most [`PER_RUN`] in a run. A failed request keeps a stale record, if any.
pub async fn lookup<F, Fut>(store: &Store, addrs: &[IpAddr], cancel: &AtomicBool, fetch: F) -> HashMap<IpAddr, Block>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let allowed = store.get::<bool>(ACCEPTED) == Some(true);
    let now = crate::bench::now_ms();
    let mut out = HashMap::new();
    let mut asked = 0;
    for &ip in addrs {
        let kept = cached(store, ip);
        if let Some(b) = kept.as_ref().filter(|b| now - b.fetched_at <= MAX_AGE_MS) {
            out.insert(ip, b.clone());
            continue;
        }
        if !allowed || asked >= PER_RUN || cancel.load(Ordering::SeqCst) {
            if let Some(b) = kept {
                out.insert(ip, b);
            }
            continue;
        }
        if asked > 0 {
            tokio::time::sleep(PAUSE).await;
        }
        asked += 1;
        let got = match tokio::time::timeout(TIMEOUT, fetch(format!("{BASE}{ip}"))).await {
            Ok(Ok(body)) => parse(&body, crate::bench::now_ms()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("timed out".into()),
        };
        match got {
            Ok(b) => {
                put(store, &b);
                out.insert(ip, b);
            }
            Err(e) => {
                eprintln!("Registry lookup for {ip} failed: {e}");
                if let Some(b) = kept {
                    out.insert(ip, b);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const RIPE: &str = r#"{"handle":"203.0.113.0 - 203.0.113.255","startAddress":"203.0.113.0","endAddress":"203.0.113.255",
        "name":"EX-BLOCK","type":"LEGACY","country":"IE","port43":"whois.ripe.net",
        "events":[{"eventAction":"registration","eventDate":"2018-12-05T15:45:57Z"},{"eventAction":"last changed","eventDate":"2022-10-29T00:51:39Z"}],
        "entities":[
          {"handle":"ex-mnt","roles":["registrant"],"vcardArray":["vcard",[["version",{},"text","4.0"],["fn",{},"text","ex-mnt"],["kind",{},"text","individual"]]]},
          {"handle":"ORG-EX1-RIPE","roles":["registrant"],"vcardArray":["vcard",[["version",{},"text","4.0"],["fn",{},"text","Example Networks Limited"],["kind",{},"text","org"]]],
           "entities":[{"handle":"NE1-RIPE","roles":["technical"],"vcardArray":["vcard",[["fn",{},"text","Network Engineering"],["kind",{},"text","group"]]]}]}
        ]}"#;
    const ARIN: &str = r#"{"handle":"NET-198-51-100-0-1","startAddress":"198.51.100.0","endAddress":"198.51.103.255",
        "name":"EXAMPLE-IX","type":"DIRECT ALLOCATION","port43":"whois.arin.net",
        "events":[{"eventAction":"last changed","eventDate":"2012-10-23T10:33:41-04:00"}],
        "entities":[{"handle":"EXAMPLE","roles":["registrant"],"vcardArray":["vcard",[["fn",{},"text","Example, Inc."],["kind",{},"text","org"]]],
          "entities":[{"handle":"P1-ARIN","roles":["technical"],"vcardArray":["vcard",[["fn",{},"text","A Person"],["kind",{},"text","individual"]]]}]}]}"#;

    #[test]
    fn a_record_reads_as_its_organisation_block_and_registry() {
        let b = parse(RIPE.as_bytes(), 0).unwrap();
        assert_eq!(
            (b.holder(), b.name.as_str(), b.rir.as_str(), b.source(), day(&b.updated)),
            ("Example Networks Limited", "EX-BLOCK", "RIPE", "RIPE registry", "2022-10-29")
        );
        assert_eq!((b.start.to_string().as_str(), b.end.to_string().as_str()), ("203.0.113.0", "203.0.113.255"));
        let a = parse(ARIN.as_bytes(), 0).unwrap();
        assert_eq!((a.holder(), a.rir.as_str(), a.handle.as_str()), ("Example, Inc.", "ARIN", "NET-198-51-100-0-1"));
    }

    #[test]
    fn a_record_without_an_organisation_falls_back_to_its_name() {
        let b = parse(br#"{"startAddress":"192.0.2.0","endAddress":"192.0.2.255","name":"TEST-NET-1"}"#, 86_400_000 * 20_000).unwrap();
        assert_eq!((b.holder(), b.rir.as_str(), b.source(), b.updated.as_str()), ("TEST-NET-1", "registry", "registry", "2024-10-04"));
        assert!(parse(br#"{"startAddress":"192.0.2.9","endAddress":"192.0.2.1"}"#, 0).is_err());
        assert!(parse(b"nope", 0).is_err());
    }

    #[test]
    fn a_number_is_owned_by_its_registrant_organisation() {
        let arin = br#"{"handle":"AS64496","name":"EXAMPLE-AS","entities":[
          {"handle":"EXAMPLE","roles":["registrant"],"vcardArray":["vcard",[["fn",{},"text","Example, Inc."],["kind",{},"text","org"]]]}]}"#;
        assert_eq!(parse_owner(arin).unwrap().as_deref(), Some("Example, Inc."));
        let nobody = br#"{"handle":"AS64496","entities":[{"roles":["registrant"],"vcardArray":["vcard",[["fn",{},"text","ex-mnt"],["kind",{},"text","individual"]]]}]}"#;
        assert_eq!(parse_owner(nobody).unwrap(), None);
        assert!(parse_owner(b"nope").is_err());
    }

    #[tokio::test]
    async fn owners_are_asked_once_and_a_blank_answer_is_kept() {
        let dir = std::env::temp_dir().join(format!("recursa-rdap-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("t.db")).unwrap();
        let cancel = AtomicBool::new(false);
        let calls = Cell::new(0);
        let fetch = |url: String| {
            calls.set(calls.get() + 1);
            async move {
                Ok(match url.as_str() {
                    "https://rdap.org/autnum/64496" => br#"{"entities":[{"roles":["registrant"],"vcardArray":["vcard",[["fn",{},"text","Example, Inc."],["kind",{},"text","org"]]]}]}"#.to_vec(),
                    "https://rdap.org/autnum/64497" => b"{}".to_vec(),
                    u => panic!("unexpected {u}"),
                })
            }
        };
        assert!(owners(&store, &[64496, 64497], &cancel, &fetch).await.is_empty());
        assert_eq!(calls.get(), 0);
        store.put(ACCEPTED, &true);
        let got = owners(&store, &[64496, 64497], &cancel, &fetch).await;
        assert_eq!(got.get(&64496).map(String::as_str), Some("Example, Inc."));
        assert_eq!((got.len(), calls.get()), (1, 2));
        let again = owners(&store, &[64496, 64497], &cancel, &fetch).await;
        assert_eq!((again.len(), calls.get()), (1, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn day(s: &str) -> &str {
        s.get(..10).unwrap_or(s)
    }

    #[tokio::test]
    async fn blocks_are_fetched_once_only_when_allowed_and_kept_for_the_whole_block() {
        let dir = std::env::temp_dir().join(format!("recursa-rdap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("t.db")).unwrap();
        let cancel = AtomicBool::new(false);
        let calls = Cell::new(0);
        let fetch = |url: String| {
            calls.set(calls.get() + 1);
            async move {
                assert_eq!(url, "https://rdap.org/ip/203.0.113.7");
                Ok(RIPE.as_bytes().to_vec())
            }
        };
        let addrs: Vec<IpAddr> = vec!["203.0.113.7".parse().unwrap(), "203.0.113.200".parse().unwrap()];
        // Not allowed: nothing is asked.
        assert!(lookup(&store, &addrs, &cancel, &fetch).await.is_empty());
        assert_eq!(calls.get(), 0);
        store.put(ACCEPTED, &true);
        let got = lookup(&store, &addrs, &cancel, &fetch).await;
        assert_eq!((calls.get(), got.len(), count(&store)), (1, 2, 1));
        assert_eq!(got[&addrs[1]].holder(), "Example Networks Limited");
        // Kept: a later map asks nothing.
        let again = lookup(&store, &addrs, &cancel, &fetch).await;
        assert_eq!((calls.get(), again.len()), (1, 2));
        assert!(cached(&store, "203.0.114.1".parse().unwrap()).is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
