//! The edge roll call: which site each CDN serves this connection from, by the CDN's own answer, with the next
//! network's name and a handshake time to the address that answered. One request per CDN over the system resolver;
//! a CDN that doesn't say shows its address and no site. Nothing is stored: the last roll call is kept in memory.

use crate::app::AppState;
use crate::bench;
use crate::cdn;
use crate::cymru;
use crate::dns;
use crate::resolve::Resolve;
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// How long one CDN gets to answer.
const TIMEOUT: Duration = Duration::from_secs(6);
/// The most of the Google report that is read; the rest is a debug blob.
const REPORT_MAX: usize = 4096;
/// Great-circle km that cost 1 ms of round trip in fibre: a rule of thumb.
pub(crate) const KM_PER_MS: f64 = 100.0;

/// Airports the CDNs use as site codes: code to (city, country, latitude, longitude).
pub(crate) static AIRPORTS: LazyLock<HashMap<String, (String, String, f64, f64)>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../data/iata.json")).expect("iata.json"));

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct You {
    pub asn: Option<u32>,
    pub holder: Option<String>,
    /// Where Cloudflare's geo-IP puts this connection, rounded to 0.1 degree. Kept in memory only.
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdgeSite {
    pub cdn: &'static str,
    pub name: &'static str,
    pub host: &'static str,
    pub method: &'static str,
    /// IATA code, upper case.
    pub site: Option<String>,
    /// What the CDN said, as received.
    pub site_raw: Option<String>,
    pub site_city: Option<String>,
    pub site_country: Option<String>,
    /// The address the request connected to.
    pub ip: Option<IpAddr>,
    pub asn: Option<u32>,
    pub holder: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RollCall {
    pub ts: i64,
    pub elapsed_ms: f64,
    pub you: You,
    pub edges: Vec<EdgeSite>,
}

/// What one request found, before the addresses are named and timed.
#[derive(Default)]
struct Got {
    site_raw: Option<String>,
    site: Option<String>,
    ip: Option<IpAddr>,
    you: Option<You>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------------------------

/// `code` as an upper-case three-letter site code.
fn iata(code: &str) -> Option<String> {
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic())).then(|| code.to_ascii_uppercase())
}

/// The letters before the first digit: `IAD61-P8` gives `IAD`, `lga25s86` gives `LGA`.
fn letters_before_digit(s: &str) -> Option<String> {
    iata(s.split(|c: char| c.is_ascii_digit()).next()?)
}

/// `x-served-by: cache-dfw-kdfw8210040-DFW, cache-iad-kiad7000123-IAD`. The last entry is the edge that served the
/// client; the ones before it are shields nearer the origin. Checked 2026-10-27 against a second Fastly host:
/// www.fastly.com listed `cache-sjc`, then `cache-iad`, and read IAD from here; pypi.org listed `cache-iad`,
/// `cache-iad`, `cache-dfw`, and a handshake to its address took 42 ms against 17 ms to developer.fastly.com (IAD),
/// so its last entry (DFW) is the edge too. The code is the suffix after the last `-`.
fn fastly_site(header: &str) -> Option<String> {
    let last = header.split(',').map(str::trim).rfind(|e| !e.is_empty())?;
    iata(last.rsplit('-').next()?)
}

/// `x-amz-cf-pop: IAD61-P8`.
fn cloudfront_site(header: &str) -> Option<String> {
    letters_before_digit(header.trim())
}

/// The first line of Google's report: `151.181.161.124 => lga25s86 (151.181.160.0/23)`. The address before the arrow
/// is the client's and is never kept.
fn google_site(body: &str) -> Option<String> {
    let line = body.lines().next()?;
    letters_before_digit(line.split_once("=> ")?.1.split_whitespace().next()?)
}

/// A site code in a reverse name: a three-letter token followed by digits, as in `edge-star-shv-01-iad3.facebook.com`.
fn ptr_site(name: &str) -> Option<String> {
    name.split(['.', '-']).find_map(|t| {
        let (code, digits) = t.split_at_checked(3)?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| iata(code)).flatten()
    })
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// You, from Cloudflare's speed-test headers. The geo-IP fields are plain names (`city`); the `cf-meta-` ones are the
/// same values as exposed to browsers, and are used when the plain ones are missing. The address header is never read.
fn cloudflare_you(h: &http::HeaderMap) -> You {
    let get = |names: [&str; 2]| names.iter().find_map(|n| h.get(*n)).and_then(|v| v.to_str().ok()).map(str::to_string);
    let num = |names: [&str; 2]| get(names).and_then(|v| v.parse::<f64>().ok()).map(round1);
    You {
        asn: get(["asn", "cf-meta-asn"]).and_then(|v| v.trim().parse().ok()),
        holder: None,
        city: get(["city", "cf-meta-city"]).filter(|c| !c.is_empty()),
        lat: num(["latitude", "cf-meta-latitude"]),
        lon: num(["longitude", "cf-meta-longitude"]),
    }
}

/// Great-circle distance in km (haversine, mean Earth radius).
pub(crate) fn great_circle_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let (dp, dl) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().asin()
}

// ---------------------------------------------------------------------------------------------
// The roll call
// ---------------------------------------------------------------------------------------------

/// The CDN as the table names it: the benchmark's labels ("anycast control") are about resolver steering, not this.
fn short_name(id: &str) -> &'static str {
    match id {
        "cloudflare" => "Cloudflare",
        "fastly" => "Fastly",
        "cloudfront" => "CloudFront",
        "google" => "Google",
        "meta" => "Meta",
        "akamai" => "Akamai",
        _ => "",
    }
}

/// Per CDN: id, the method that names its site, the property asked.
const ASKED: [(&str, &str, &str); 6] = [
    ("cloudflare", "cf-headers", "speed.cloudflare.com"),
    ("fastly", "x-served-by", "pypi.org"),
    ("cloudfront", "x-amz-cf-pop", "d1.awsstatic.com"),
    ("google", "report_mapping", "redirector.googlevideo.com"),
    ("meta", "fb-rdns", "www.facebook.com"),
    ("akamai", "none", "www.microsoft.com"),
];

fn header(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// The request for one CDN: its answer, and the address it connected to.
async fn ask(client: &reqwest::Client, id: &str, host: &str) -> Got {
    let url = match id {
        "cloudflare" => format!("https://{host}/__down?bytes=0"),
        "google" => format!("https://{host}/report_mapping"),
        _ => format!("https://{host}/"),
    };
    let mut req = if id == "google" { client.get(&url) } else { client.head(&url) };
    if id == "fastly" {
        req = req.header("Fastly-Debug", "1");
    }
    let mut r = match req.send().await {
        Ok(r) => r,
        Err(e) => return Got { error: Some(if e.is_timeout() { "timed out" } else { "no answer" }.into()), ..Default::default() },
    };
    let mut got = Got { ip: r.remote_addr().map(|a| a.ip()), ..Default::default() };
    match id {
        "cloudflare" => {
            got.site_raw = header(&r, "colo");
            got.site = got.site_raw.as_deref().and_then(|c| iata(c.trim()));
            got.you = Some(cloudflare_you(r.headers()));
        }
        "fastly" => {
            got.site_raw = header(&r, "x-served-by");
            got.site = got.site_raw.as_deref().and_then(fastly_site);
        }
        "cloudfront" => {
            got.site_raw = header(&r, "x-amz-cf-pop");
            got.site = got.site_raw.as_deref().and_then(cloudfront_site);
        }
        "google" => {
            let mut body = Vec::new();
            while body.len() < REPORT_MAX
                && let Ok(Some(chunk)) = r.chunk().await
            {
                body.extend_from_slice(&chunk);
            }
            body.truncate(REPORT_MAX);
            let text = String::from_utf8_lossy(&body);
            // The token alone: the address before the arrow and the prefix after it are the client's.
            got.site_raw = text
                .lines()
                .next()
                .and_then(|l| l.split_once("=> "))
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .map(str::to_string);
            got.site = google_site(&text);
        }
        _ => {}
    }
    got
}

/// The PTR name of `ip`, when it has one.
pub(crate) fn ptr(res: &Resolve, ip: IpAddr) -> Option<String> {
    let name = match ip.to_canonical() {
        IpAddr::V4(a) => {
            let o = a.octets();
            format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(a) => format!("{}.ip6.arpa", dns::nibbles(a)),
    };
    let m = res.query(&name, dns::TYPE_PTR).ok()?;
    m.answers.iter().find_map(|r| if let dns::RData::Name(n) = &r.data { Some(n.trim_end_matches('.').to_string()) } else { None })
}

/// Name the addresses (Cymru), read Meta's reverse name, time the handshakes, and work out each floor. Blocks.
fn finish(res: &Resolve, you: &mut You, edges: &mut [EdgeSite]) {
    let holder = |asn: u32| cymru::name_of(res, asn).ok().flatten().map(|n| cymru::holder_name(&n));
    if let (Some(asn), None) = (you.asn, &you.holder) {
        you.holder = holder(asn);
    }
    for e in edges.iter_mut() {
        let Some(ip) = e.ip else { continue };
        if let Ok(Some(o)) = cymru::origin_of(res, ip) {
            e.asn = Some(o.asn);
            e.holder = holder(o.asn);
        }
        if e.cdn == "meta" {
            let name = ptr(res, ip);
            e.site = name.as_deref().and_then(ptr_site);
            e.site_raw = name;
        }
    }
    for e in edges.iter_mut() {
        if let Some((city, country, ..)) = e.site.as_deref().and_then(|s| AIRPORTS.get(s)) {
            e.site_city = Some(city.clone());
            e.site_country = Some(country.clone());
        }
    }
}

/// Ask the six CDNs which site serves this connection. Takes a few seconds.
pub async fn roll_call(app: &AppState) -> RollCall {
    let t0 = Instant::now();
    let ts = bench::now_ms();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Recursa-DNS-Benchmark/", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        // The peer must be the host asked, not wherever it sends us.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client");
    let got = futures_util::future::join_all(ASKED.iter().map(|(id, _, host)| ask(&client, id, host))).await;
    let mut you = got.iter().find_map(|g| g.you.clone()).unwrap_or_default();
    if you.asn.is_none() {
        let v = app.vantage.read().unwrap();
        you.asn = v["network"]["asn"].as_u64().map(|a| a as u32);
        you.holder = v["network"]["holder"].as_str().map(str::to_string);
    }
    let mut edges: Vec<EdgeSite> = ASKED
        .iter()
        .zip(got)
        .map(|((id, method, host), g)| EdgeSite {
            cdn: cdn::CDNS.iter().find(|c| c.id == *id).map_or("", |c| c.id),
            name: short_name(id),
            host,
            method,
            site: g.site,
            site_raw: g.site_raw,
            site_city: None,
            site_country: None,
            ip: g.ip,
            asn: None,
            holder: None,
            error: g.error,
        })
        .collect();
    let res = Resolve::new(app.system.read().unwrap().clone());
    let (you, edges) = tokio::task::spawn_blocking(move || {
        finish(&res, &mut you, &mut edges);
        (you, edges)
    })
    .await
    .unwrap_or_default();
    RollCall { ts, elapsed_ms: round1(t0.elapsed().as_secs_f64() * 1000.0), you, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastly_edge_is_the_last_entry() {
        let h = "cache-iad-khef600071-IAD, cache-iad-khef600051-IAD, cache-dfw-kdfw8210059-DFW";
        assert_eq!(fastly_site(h).as_deref(), Some("DFW"));
        assert_eq!(fastly_site("cache-sjc1000097-SJC, cache-iad-kiad7000160-IAD").as_deref(), Some("IAD"));
        assert_eq!(fastly_site("cache-dfw-kdfw8210040-DFW").as_deref(), Some("DFW"));
        assert_eq!(fastly_site(""), None);
        assert_eq!(fastly_site("cache-x-1234"), None);
    }

    #[test]
    fn cloudfront_pop_letters() {
        assert_eq!(cloudfront_site("IAD61-P8").as_deref(), Some("IAD"));
        assert_eq!(cloudfront_site("JFK52-P2").as_deref(), Some("JFK"));
        assert_eq!(cloudfront_site("Miss"), None);
    }

    #[test]
    fn google_report_first_line_only() {
        let body = "151.181.161.124 => lga25s86 (151.181.160.0/23)\n\nDebug Info:\no-AGh1RpMg7WFVJxFZXG55\nm6yucKwBFQ";
        assert_eq!(google_site(body).as_deref(), Some("LGA"));
        assert_eq!(google_site("no arrow here\nx => iad1"), None);
        assert_eq!(google_site(""), None);
    }

    #[test]
    fn reverse_name_site_needs_letters_then_digit() {
        assert_eq!(ptr_site("edge-star-shv-01-iad3.facebook.com").as_deref(), Some("IAD"));
        assert_eq!(ptr_site("a.b.c.lga1.example.net").as_deref(), Some("LGA"));
        assert_eq!(ptr_site("star-mini.c10r.facebook.com"), None);
        assert_eq!(ptr_site("iad.example.net"), None);
    }

    fn headers(pairs: &[(&str, &str)]) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    #[test]
    fn cloudflare_headers_with_and_without_geo() {
        let y = cloudflare_you(&headers(&[("asn", "46887"), ("city", "Charlotte"), ("latitude", "35.22709"), ("longitude", "-80.84313")]));
        assert_eq!((y.asn, y.city.as_deref(), y.lat, y.lon), (Some(46887), Some("Charlotte"), Some(35.2), Some(-80.8)));
        let y = cloudflare_you(&headers(&[
            ("cf-meta-asn", "7018"),
            ("cf-meta-city", "Dallas"),
            ("cf-meta-latitude", "32.84"),
            ("cf-meta-longitude", "-96.95"),
        ]));
        assert_eq!((y.asn, y.city.as_deref(), y.lat, y.lon), (Some(7018), Some("Dallas"), Some(32.8), Some(-97.0)));
        let y = cloudflare_you(&headers(&[("asn", "46887"), ("cf-meta-ip", "203.0.113.9")]));
        assert_eq!((y.asn, y.city, y.lat, y.lon), (Some(46887), None, None, None));
    }

    #[test]
    fn every_airport_row_is_in_range() {
        for (code, (_, _, lat, lon)) in AIRPORTS.iter() {
            assert!(code.len() == 3 && (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon), "{code}");
        }
    }

    /// Against the real network: `cargo test -p recursa-engine live_roll_call --release -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_roll_call() {
        crate::install_crypto_provider();
        let db = std::env::temp_dir().join(format!("recursa-edge-{:x}.db", fastrand::u64(..)));
        let app = AppState::open(Some(db), false, crate::caps::Caps::BROWSER).unwrap();
        let rc = roll_call(&app).await;
        println!("you: {:?} ({} ms)", rc.you, rc.elapsed_ms);
        for e in &rc.edges {
            println!(
                "{:<10} {:<4} {:<12} raw={:?} ip={:?} AS{:?} {:?} err={:?}",
                e.cdn,
                e.site.as_deref().unwrap_or("-"),
                e.site_city.as_deref().unwrap_or("-"),
                e.site_raw,
                e.ip,
                e.asn,
                e.holder,
                e.error
            );
        }
    }
}
