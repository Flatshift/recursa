//! IP address to origin AS, BGP prefix and AS name, from Team Cymru's DNS service.
//!
//! * `<reversed-ip>.origin.asn.cymru.com` TXT (IPv4), `<nibbles>.origin6.asn.cymru.com` TXT (IPv6):
//!   `"15169 | 8.8.8.0/24 | US | arin | 2023-12-28"`
//! * `AS15169.asn.cymru.com` TXT: `"15169 | US | arin | 2000-03-30 | GOOGLE - Google LLC, US"`
//!
//! Answers are cached per address for the life of the process; "not announced" and malformed
//! answers are asked again after an hour.
//! Only addresses the app has a reason to name are ever asked about: the user's
//! own public address, resolver and CDN addresses. Used by [`crate::intel`].

use crate::dns;
use crate::resolve::Resolve;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How long a missing or malformed answer is believed.
const NEGATIVE_TTL: Duration = Duration::from_secs(3600);
const CACHE_MAX: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub asn: u32,
    pub prefix: String,
    pub country: String,
}

type Cache<K, V> = LazyLock<Mutex<HashMap<K, (Option<V>, Instant)>>>;
static ORIGINS: Cache<IpAddr, Origin> = LazyLock::new(Default::default);
static NAMES: Cache<u32, String> = LazyLock::new(Default::default);

fn cached<K: std::hash::Hash + Eq, V: Clone>(cache: &Cache<K, V>, key: &K) -> Option<Option<V>> {
    let c = cache.lock().unwrap();
    let (v, at) = c.get(key)?;
    (v.is_some() || at.elapsed() < NEGATIVE_TTL).then(|| v.clone())
}

fn remember<K: std::hash::Hash + Eq, V>(cache: &Cache<K, V>, key: K, v: Option<V>) {
    let mut c = cache.lock().unwrap();
    if c.len() > CACHE_MAX {
        c.clear();
    }
    c.insert(key, (v, Instant::now()));
}

/// `"15169 | 8.8.8.0/24 | US | arin | 2023-12-28"`. When a prefix has several origins
/// (`"13335 32934 | ..."`) the first is used.
fn parse_origin(txt: &str) -> Option<Origin> {
    let mut f = txt.split('|').map(str::trim);
    let asn = f.next()?.split_whitespace().next()?.parse().ok()?;
    let prefix = f.next()?.to_string();
    let country = f.next().unwrap_or("").to_string();
    Some(Origin { asn, prefix, country })
}

/// `"15169 | US | arin | 2000-03-30 | GOOGLE, US"` gives `GOOGLE, US`.
fn parse_as_name(txt: &str) -> Option<String> {
    let name = txt.split('|').nth(4)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// An AS name as the UI's `holder()` expects it: `"CLOUDFLARENET - Cloudflare, Inc., US"` loses the
/// registry's trailing country code and reads `"CLOUDFLARENET - Cloudflare, Inc."`.
pub fn holder_name(name: &str) -> String {
    match name.rsplit_once(", ") {
        Some((rest, cc)) if cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase()) => rest.to_string(),
        _ => name.to_string(),
    }
}

fn origin_query_name(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(a) => {
            let o = a.octets();
            format!("{}.{}.{}.{}.origin.asn.cymru.com", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(a) => format!("{}.origin6.asn.cymru.com", dns::nibbles(a)),
    }
}

/// One TXT lookup: `Ok(None)` for NXDOMAIN (not announced), `Err` for failures worth retrying later.
fn txt(res: &Resolve, name: &str) -> Result<Option<String>, String> {
    let m = res.query(name, dns::TYPE_TXT)?;
    match m.rcode {
        dns::RCODE_NOERROR => Ok(m.txt_answers().into_iter().next()),
        dns::RCODE_NXDOMAIN => Ok(None),
        rc => Err(dns::rcode_name(rc).to_string()),
    }
}

/// The origin of `ip`: `Ok(None)` when no one announces it, `Err` when the question failed.
pub fn origin_of(res: &Resolve, ip: IpAddr) -> Result<Option<Origin>, String> {
    let ip = ip.to_canonical();
    if let Some(hit) = cached(&ORIGINS, &ip) {
        return Ok(hit);
    }
    let found = txt(res, &origin_query_name(ip))?.as_deref().and_then(parse_origin);
    remember(&ORIGINS, ip, found.clone());
    Ok(found)
}

/// The registered name of an AS: `Ok(None)` when it has none, `Err` when the question failed.
pub fn name_of(res: &Resolve, asn: u32) -> Result<Option<String>, String> {
    if let Some(hit) = cached(&NAMES, &asn) {
        return Ok(hit);
    }
    let found = txt(res, &format!("AS{asn}.asn.cymru.com"))?.as_deref().and_then(parse_as_name);
    remember(&NAMES, asn, found.clone());
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_answers() {
        let o = parse_origin("15169 | 8.8.8.0/24 | US | arin | 2023-12-28").unwrap();
        assert_eq!((o.asn, o.prefix.as_str(), o.country.as_str()), (15169, "8.8.8.0/24", "US"));
        let o = parse_origin("13335 32934 | 1.1.1.0/24 | AU | apnic | 2011-08-11").unwrap();
        assert_eq!(o.asn, 13335);
        // IPv6 answers have the same shape.
        let o = parse_origin("13335 | 2606:4700::/32 | US | arin | 2011-02-02").unwrap();
        assert_eq!((o.asn, o.prefix.as_str()), (13335, "2606:4700::/32"));
        assert!(parse_origin("").is_none());
        assert!(parse_origin("NA | 10.0.0.0/8").is_none());
    }

    #[test]
    fn name_answers() {
        assert_eq!(parse_as_name("15169 | US | arin | 2000-03-30 | GOOGLE, US").as_deref(), Some("GOOGLE, US"));
        assert_eq!(parse_as_name("15169 | US | arin | 2000-03-30 | ").as_deref(), None);
        assert_eq!(parse_as_name("garbage"), None);
    }

    #[test]
    fn holders_read_like_the_old_registry_form() {
        assert_eq!(holder_name("EXAMPLENET-AS64500 - Example Networks LLC, US"), "EXAMPLENET-AS64500 - Example Networks LLC");
        assert_eq!(holder_name("CLOUDFLARENET - Cloudflare, Inc., US"), "CLOUDFLARENET - Cloudflare, Inc.");
        assert_eq!(holder_name("GOOGLE, US"), "GOOGLE");
        // Nothing that isn't a country code is cut.
        assert_eq!(holder_name("EXAMPLE-NET"), "EXAMPLE-NET");
        assert_eq!(holder_name("Acme, Inc."), "Acme, Inc.");
    }

    #[test]
    fn misses_expire_but_answers_stay() {
        static C: Cache<u8, u8> = LazyLock::new(Default::default);
        remember(&C, 1, Some(7));
        remember(&C, 2, None);
        assert_eq!((cached(&C, &1), cached(&C, &2), cached(&C, &3)), (Some(Some(7)), Some(None), None));
        C.lock().unwrap().get_mut(&2).unwrap().1 -= NEGATIVE_TTL;
        C.lock().unwrap().get_mut(&1).unwrap().1 -= NEGATIVE_TTL;
        assert_eq!((cached(&C, &1), cached(&C, &2)), (Some(Some(7)), None));
    }

    #[test]
    fn query_names() {
        assert_eq!(origin_query_name("8.8.4.4".parse().unwrap()), "4.4.8.8.origin.asn.cymru.com");
        let v6 = origin_query_name("2001:db8::1".parse().unwrap());
        assert!(v6.starts_with("1.0.0.0.") && v6.ends_with(".8.b.d.0.1.0.0.2.origin6.asn.cymru.com"));
    }
}
