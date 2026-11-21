//! The two downloaded tables: where they come from, how a download is read into a [`Table`], and the checks a
//! download passes before it replaces the saved one.

use super::table::{Table, Vrp};
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::io::{BufReader, Read};
use std::net::IpAddr;
use std::time::Duration;

/// A table is used, and a download accepted, only while its build time is under this old.
pub(super) const MAX_AGE_MS: i64 = 24 * 3_600_000;
/// How far ahead of this computer's clock a build time may be.
const MAX_AHEAD_MS: i64 = 10 * 60_000;
/// A download with fewer entries than this is not a whole table.
pub(super) const MIN_ENTRIES: usize = 500_000;
/// The compressed (or plain) body is held in memory up to this size.
const MAX_BODY: usize = 32 << 20;

/// One public validator's export.
pub(super) struct Def {
    pub key: &'static str,
    pub name: &'static str,
    pub engine: &'static str,
    pub url: &'static str,
    /// `metadata` members that hold the build time, first match wins.
    time_fields: &'static [&'static str],
}

/// The first is the primary: its ROAs are listed in a result.
pub(super) const DEFS: [Def; 2] = [
    Def {
        key: "console",
        name: "rpki-client console",
        engine: "rpki-client",
        url: "https://console.rpki-client.org/rpki.json",
        time_fields: &["buildtime"],
    },
    Def {
        key: "ripe",
        name: "RIPE NCC",
        engine: "Routinator",
        url: "https://rpki-validator.ripe.net/api/export.json",
        time_fields: &["generatedTime", "generated"],
    },
];

impl Def {
    pub(super) fn file(&self) -> String {
        format!("feed-{}.bin", self.key)
    }
}

pub(super) enum Failure {
    /// The setting went off or a measured phase began.
    Stopped,
    Failed(String),
}

/// What one request brought back.
pub(super) enum Reply {
    NotModified,
    Body { bytes: Vec<u8>, gzip: bool, last_modified: Option<String> },
}

/// A download read to its end.
pub(super) struct Parsed {
    vrps: Vec<Vrp>,
    v4: usize,
    v6: usize,
    /// ms since the epoch.
    build_time: Option<i64>,
}

#[derive(Default)]
struct Acc {
    vrps: Vec<Vrp>,
    v4: usize,
    v6: usize,
    build_time: Option<i64>,
}

/// An origin AS: a number, or text like `AS64496`. `None` when it isn't one that fits.
struct Asn(Option<u32>);

impl<'de> Deserialize<'de> for Asn {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Asn;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an AS number")
            }
            fn visit_u64<E>(self, n: u64) -> Result<Asn, E> {
                Ok(Asn(u32::try_from(n).ok()))
            }
            fn visit_str<E>(self, s: &str) -> Result<Asn, E> {
                let digits = s.strip_prefix("AS").or_else(|| s.strip_prefix("as")).unwrap_or(s);
                Ok(Asn(digits.parse().ok()))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct Roa {
    asn: Asn,
    prefix: String,
    #[serde(rename = "maxLength")]
    max_length: Option<u8>,
}

impl Acc {
    /// Adds one entry; one that doesn't read as a prefix, an AS and a sane maximum length is left out.
    fn add(&mut self, r: Roa) {
        let Some(asn) = r.asn.0 else { return };
        let Some((addr, len)) = r.prefix.split_once('/').and_then(|(a, l)| Some((a.parse::<IpAddr>().ok()?, l.parse::<u8>().ok()?))) else {
            return;
        };
        let bits = if addr.is_ipv4() { 32 } else { 128 };
        let max_len = r.max_length.unwrap_or(len);
        if len > bits || max_len < len || max_len > bits {
            return;
        }
        if addr.is_ipv4() {
            self.v4 += 1;
        } else {
            self.v6 += 1;
        }
        self.vrps.push(Vrp { addr, len, max_len, asn });
    }
}

/// The `roas` array, one entry at a time.
struct Roas<'a>(&'a mut Acc);

impl<'de> DeserializeSeed<'de> for Roas<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Roas<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of ROAs")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(r) = seq.next_element::<Roa>()? {
            self.0.add(r);
        }
        Ok(())
    }
}

/// The top-level object: `roas` is consumed as it streams past, `metadata` is read for the build time, the rest is skipped.
struct Doc<'a>(&'a Def, &'a mut Acc);

impl<'de> DeserializeSeed<'de> for Doc<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Doc<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object with `roas`")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let Doc(def, acc) = self;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "roas" => map.next_value_seed(Roas(acc))?,
                "metadata" => {
                    let m: serde_json::Value = map.next_value()?;
                    acc.build_time = def.time_fields.iter().find_map(|f| to_ms(&m[*f]));
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// A build time: RFC 3339 text, or Unix seconds as a number or as text.
fn to_ms(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::String(s) => rfc3339_ms(s).or_else(|| s.parse::<i64>().ok().map(|n| n * 1000)),
        serde_json::Value::Number(n) => n.as_i64().map(|n| n * 1000),
        _ => None,
    }
}

/// `2026-11-21T15:20:00Z` (fraction and numeric offsets allowed) as ms since the epoch.
fn rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = b.get(from..to)?;
        part.iter().all(u8::is_ascii_digit).then(|| part.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut millis = 0;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        millis = num(start, (start + 3).min(i))? * 10i64.pow(3 - (i - start).min(3) as u32);
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let o = (num(i + 1, i + 3)? * 60 + num(i + 4, i + 6)?) * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // Days since 1970-01-01 (Howard Hinnant's days-from-civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days * 24 + h) * 3600 + mi * 60 + sec - offset) * 1000 + millis)
}

/// Reads one export, streaming: the entries of `roas` are collected as they are decoded, and the decoded text is never held.
pub(super) fn parse(def: &Def, bytes: &[u8], gzip: bool) -> Result<Parsed, String> {
    let mut acc = Acc::default();
    let reader: Box<dyn Read + '_> = if gzip { Box::new(flate2::read::GzDecoder::new(bytes)) } else { Box::new(bytes) };
    let mut de = serde_json::Deserializer::from_reader(BufReader::with_capacity(64 << 10, reader));
    Doc(def, &mut acc).deserialize(&mut de).and_then(|()| de.end()).map_err(|e| format!("The table could not be read: {e}"))?;
    Ok(Parsed { vrps: acc.vrps, v4: acc.v4, v6: acc.v6, build_time: acc.build_time })
}

/// The gates of a download: the table and its build time (ms), or why it is refused. `previous` is the size of the
/// table it would replace; `floor` is the fewest entries a table may have.
pub(super) fn accept(p: Parsed, previous: Option<usize>, now: i64, floor: usize) -> Result<(Table, i64), String> {
    if p.v4 == 0 || p.v6 == 0 {
        return Err("The table has no entries for one address family".into());
    }
    let Some(built) = p.build_time else { return Err("The table has no build time".into()) };
    if now - built >= MAX_AGE_MS {
        return Err("The table is over 24 hours old".into());
    }
    if built - now > MAX_AHEAD_MS {
        return Err("The table's build time is in the future".into());
    }
    let table = Table::new(p.vrps);
    if table.len() < floor {
        return Err(format!("The table has only {} entries", table.len()));
    }
    if previous.is_some_and(|prev| table.len() * 10 < prev * 9) {
        return Err(format!("The table shrank from {} to {} entries", previous.unwrap_or(0), table.len()));
    }
    Ok((table, built))
}

/// `f`, given up once `stop` says so: a server can take most of a minute over its first byte.
async fn unless_stopped<T>(f: impl Future<Output = T>, stop: &dyn Fn() -> bool) -> Result<T, Failure> {
    tokio::pin!(f);
    loop {
        tokio::select! {
            out = &mut f => return Ok(out),
            () = tokio::time::sleep(Duration::from_millis(100)) => {
                if stop() {
                    return Err(Failure::Stopped);
                }
            }
        }
    }
}

/// HTTP for the real downloads: a client, and a runtime of its own on the sync thread.
pub(super) struct Http {
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl Http {
    pub(super) fn new() -> Result<Http, String> {
        let rt =
            tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("Couldn't start a download: {e}"))?;
        let client = reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("Couldn't start a download: {e}"))?;
        Ok(Http { rt, client })
    }

    /// One request. `since` is the `Last-Modified` of the table already held. Polls `stop` while it waits for the server.
    pub(super) fn fetch(&self, def: &Def, since: Option<&str>, stop: &dyn Fn() -> bool) -> Result<Reply, Failure> {
        let failed = |e: &dyn std::fmt::Display| Failure::Failed(e.to_string());
        self.rt.block_on(async {
            let mut req = self.client.get(def.url).header(reqwest::header::ACCEPT_ENCODING, "gzip");
            if let Some(t) = since {
                req = req.header(reqwest::header::IF_MODIFIED_SINCE, t);
            }
            let mut r = unless_stopped(req.send(), stop).await?.map_err(|e| failed(&e))?;
            if r.status() == reqwest::StatusCode::NOT_MODIFIED {
                return Ok(Reply::NotModified);
            }
            if !r.status().is_success() {
                return Err(Failure::Failed(format!("The server answered {}", r.status())));
            }
            let text = |name| r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
            let gzip = text(reqwest::header::CONTENT_ENCODING).is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
            let last_modified = text(reqwest::header::LAST_MODIFIED);
            if r.content_length().is_some_and(|n| n as usize > MAX_BODY) {
                return Err(Failure::Failed("The download is too large".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = unless_stopped(r.chunk(), stop).await?.map_err(|e| failed(&e))? {
                if bytes.len() + chunk.len() > MAX_BODY {
                    return Err(Failure::Failed("The download is too large".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Reply::Body { bytes, gzip, last_modified })
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::Write;

    /// A document in the shape of `def`'s export, gzip-compressed: `entries` as (prefix, max length, AS), the build time as given.
    pub fn document(def: &Def, entries: &[(&str, u8, u32)], built: Option<&str>) -> Vec<u8> {
        let roas: Vec<_> = entries
            .iter()
            .map(|(p, m, a)| {
                if def.key == "console" {
                    serde_json::json!({ "asn": a, "prefix": p, "maxLength": m, "ta": "x", "expires": 1 })
                } else {
                    serde_json::json!({ "asn": format!("AS{a}"), "prefix": p, "maxLength": m, "ta": "x" })
                }
            })
            .collect();
        let mut doc = serde_json::json!({ "roas": roas, "aspas": [{ "x": 1 }], "bgpsec_keys": [] });
        if let Some(t) = built {
            doc["metadata"] = serde_json::json!({ def.time_fields[0]: t, "counts": 3 });
        }
        gzip(doc.to_string().as_bytes())
    }

    pub fn gzip(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    pub const ENTRIES: [(&str, u8, u32); 3] = [("192.0.2.0/24", 24, 64496), ("198.51.100.0/24", 25, 64497), ("2001:db8::/32", 48, 64496)];

    #[test]
    fn a_stop_ends_a_request_the_server_has_not_answered() {
        // Accepts the connection (the backlog does) and never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url: &'static str = Box::leak(format!("http://{}/", silent.local_addr().unwrap()).into_boxed_str());
        let def = Def { key: "x", name: "x", engine: "x", url, time_fields: &[] };
        crate::install_crypto_provider();
        let began = std::time::Instant::now();
        let got = Http::new().unwrap().fetch(&def, None, &|| true);
        assert!(matches!(got, Err(Failure::Stopped)));
        assert!(began.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn times_are_read_from_text_and_numbers() {
        assert_eq!(rfc3339_ms("2026-11-21T15:20:00Z"), Some(1_795_274_400_000));
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(rfc3339_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(rfc3339_ms("yesterday"), None);
        assert_eq!(to_ms(&serde_json::json!(1_795_274_400)), Some(1_795_274_400_000));
        assert_eq!(to_ms(&serde_json::json!("1795274400")), Some(1_795_274_400_000));
        assert_eq!(to_ms(&serde_json::json!(null)), None);
    }

    #[test]
    fn both_shapes_give_the_same_table() {
        let when = Some("2026-11-21T15:20:00Z");
        let mut tables = Vec::new();
        for def in &DEFS {
            let p = parse(def, &document(def, &ENTRIES, when), true).unwrap();
            assert_eq!((p.v4, p.v6, p.build_time), (2, 1, Some(1_795_274_400_000)), "{}", def.key);
            tables.push(Table::new(p.vrps).to_bytes(0));
        }
        assert_eq!(tables[0], tables[1]);
    }

    #[test]
    fn ripe_falls_back_to_the_epoch_seconds_and_plain_bodies_parse() {
        let doc = br#"{"metadata":{"generated":1795274400},"roas":[{"asn":"AS64496","prefix":"192.0.2.0/24","maxLength":24}]}"#;
        let p = parse(&DEFS[1], doc, false).unwrap();
        assert_eq!((p.build_time, p.vrps.len()), (Some(1_795_274_400_000), 1));
        // The roas may come before the metadata.
        let doc = br#"{"roas":[],"metadata":{"buildtime":"2026-11-21T15:20:00Z"}}"#;
        assert_eq!(parse(&DEFS[0], doc, false).unwrap().build_time, Some(1_795_274_400_000));
    }

    #[test]
    fn a_truncated_or_broken_document_is_rejected() {
        let whole = document(&DEFS[0], &ENTRIES, Some("2026-11-21T15:20:00Z"));
        assert!(parse(&DEFS[0], &whole[..whole.len() - 10], true).is_err());
        assert!(parse(&DEFS[0], b"{\"roas\":[{\"asn\":1", false).is_err());
        assert!(parse(&DEFS[0], b"not json", false).is_err());
        assert!(parse(&DEFS[0], b"[]", false).is_err());
    }

    fn parsed(entries: &[(&str, u8, u32)], built: Option<&str>) -> Parsed {
        parse(&DEFS[0], &document(&DEFS[0], entries, built), true).unwrap()
    }

    #[test]
    fn the_gates_refuse_what_they_should() {
        let built = 1_795_274_400_000;
        let now = built + 3_600_000;
        let fresh = || parsed(&ENTRIES, Some("2026-11-21T15:20:00Z"));
        let (t, time) = accept(fresh(), None, now, 3).unwrap();
        assert_eq!((t.len(), time), (3, built));
        // Old, missing and future build times.
        assert!(accept(fresh(), None, built + MAX_AGE_MS, 3).is_err());
        assert!(accept(parsed(&ENTRIES, None), None, now, 3).is_err());
        assert!(accept(fresh(), None, built - 11 * 60_000, 3).is_err());
        // Under the floor.
        assert!(accept(fresh(), None, now, 4).is_err());
        // One address family missing.
        assert!(accept(parsed(&ENTRIES[..2], Some("2026-11-21T15:20:00Z")), None, now, 1).is_err());
        // More than 10% smaller than the table it would replace.
        assert!(accept(fresh(), Some(4), now, 3).is_err());
        assert!(accept(fresh(), Some(3), now, 3).is_ok());
    }

    #[test]
    fn entries_that_do_not_read_are_left_out() {
        let doc = br#"{"metadata":{"buildtime":"2026-11-21T15:20:00Z"},"roas":[
            {"asn":"AS64496","prefix":"192.0.2.0/24","maxLength":24},
            {"asn":"bad","prefix":"192.0.2.0/24","maxLength":24},
            {"asn":64496,"prefix":"192.0.2.0/33","maxLength":33},
            {"asn":64496,"prefix":"192.0.2.0/24","maxLength":20},
            {"asn":64496,"prefix":"2001:db8::/32"}]}"#;
        let p = parse(&DEFS[0], doc, false).unwrap();
        assert_eq!((p.v4, p.v6), (1, 1));
    }
}
