//! The edge roll call: which site each CDN serves this connection from, by the CDN's own answer, with the network the
//! answering address belongs to. One request per CDN over the system resolver; a CDN that doesn't say shows its address
//! and no site. Nothing is stored: the last roll call is kept in memory.

use crate::app::AppState;
use crate::cdn;
use crate::cymru;
use crate::dns;
use crate::resolve::Resolve;
use crate::stats::round1;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

/// How long one CDN gets to answer.
const TIMEOUT: Duration = Duration::from_secs(6);
/// The most of the Google report that is read; the rest is a debug blob.
const REPORT_MAX: usize = 4096;
/// Great-circle km that cost 1 ms of round trip in fibre: a rule of thumb.
pub(crate) const KM_PER_MS: f64 = 100.0;

/// Airports the CDNs use as site codes: code to (city, country, latitude, longitude).
pub(crate) static AIRPORTS: LazyLock<HashMap<String, (String, String, f64, f64)>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../data/iata.json")).expect("iata.json"));

#[derive(Clone, Debug, Default, PartialEq)]
pub struct You {
    pub asn: Option<u32>,
    pub holder: Option<String>,
    /// Where Cloudflare's geo-IP puts this connection, rounded to 0.1 degree. Kept in memory only.
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct EdgeSite {
    pub cdn: &'static str,
    /// IATA code, upper case.
    pub site: Option<String>,
    pub site_city: Option<String>,
    /// The address the request connected to.
    pub ip: Option<IpAddr>,
    pub asn: Option<u32>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RollCall {
    pub you: You,
    pub edges: Vec<EdgeSite>,
}

/// What one request found, before its address is looked up.
#[derive(Default)]
struct Got {
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
/// client; the ones before it are shields nearer the origin. Checked 2026-11-01 over 996 connections to 10 Fastly
/// hosts: it was always the first delivery node in `Fastly-Debug-Path`, and the handshake time fit it. From a client
/// near IAD, www.fastly.com listed `cache-sjc`, then `cache-iad`, and took 15 ms; pypi.org listed `cache-iad` twice,
/// then `cache-mfe`, and took 56 ms. The code is the suffix after the last `-`.
fn fastly_site(header: &str) -> Option<String> {
    let last = header.split(',').map(str::trim).rfind(|e| !e.is_empty())?;
    iata(last.rsplit('-').next()?)
}

/// `x-amz-cf-pop: IAD61-P8`.
fn cloudfront_site(header: &str) -> Option<String> {
    letters_before_digit(header.trim())
}

/// The first line of Google's report: `203.0.113.124 => lga25s86 (203.0.113.0/24)`. The address before the arrow
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

/// Per CDN: id, the property asked. The map traces the address the roll call connected to, with ICMP, so the property
/// has to be answered where that trace ends. Fastly's debug page is. pypi.org isn't: its connections took 56 to 80 ms,
/// one hop past the 14 ms ingress the trace stops at, and named McAllen or Vancouver.
const ASKED: [(&str, &str); 6] = [
    ("cloudflare", "speed.cloudflare.com"),
    ("fastly", "www.fastly-debug.com"),
    ("cloudfront", "d1.awsstatic.com"),
    ("google", "redirector.googlevideo.com"),
    ("meta", "www.facebook.com"),
    ("akamai", "www.microsoft.com"),
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
            got.site = header(&r, "colo").as_deref().and_then(|c| iata(c.trim()));
            got.you = Some(cloudflare_you(r.headers()));
        }
        "fastly" => got.site = header(&r, "x-served-by").as_deref().and_then(fastly_site),
        "cloudfront" => got.site = header(&r, "x-amz-cf-pop").as_deref().and_then(cloudfront_site),
        "google" => {
            let mut body = Vec::new();
            while body.len() < REPORT_MAX
                && let Ok(Some(chunk)) = r.chunk().await
            {
                body.extend_from_slice(&chunk);
            }
            body.truncate(REPORT_MAX);
            got.site = google_site(&String::from_utf8_lossy(&body));
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

/// Find each address's network (Cymru), read Meta's reverse name, and look up each site's city. Blocks.
fn finish(res: &Resolve, you: &mut You, edges: &mut [EdgeSite]) {
    if let (Some(asn), None) = (you.asn, &you.holder) {
        you.holder = cymru::name_of(res, asn).ok().flatten().map(|n| cymru::holder_name(&n));
    }
    for e in edges.iter_mut() {
        let Some(ip) = e.ip else { continue };
        if let Ok(Some(o)) = cymru::origin_of(res, ip) {
            e.asn = Some(o.asn);
        }
        if e.cdn == "meta" {
            e.site = ptr(res, ip).as_deref().and_then(ptr_site);
        }
    }
    for e in edges.iter_mut() {
        if let Some((city, ..)) = e.site.as_deref().and_then(|s| AIRPORTS.get(s)) {
            e.site_city = Some(city.clone());
        }
    }
}

/// Ask the six CDNs which site serves this connection. Takes a few seconds.
pub async fn roll_call(app: &AppState) -> RollCall {
    let client = reqwest::Client::builder()
        .user_agent(crate::USER_AGENT)
        .timeout(TIMEOUT)
        // The peer must be the host asked, not wherever it sends us.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client");
    let got = futures_util::future::join_all(ASKED.iter().map(|(id, host)| ask(&client, id, host))).await;
    let mut you = got.iter().find_map(|g| g.you.clone()).unwrap_or_default();
    if you.asn.is_none() {
        let v = app.vantage.read().unwrap();
        you.asn = v["network"]["asn"].as_u64().map(|a| a as u32);
        you.holder = v["network"]["holder"].as_str().map(str::to_string);
    }
    let mut edges: Vec<EdgeSite> = ASKED
        .iter()
        .zip(got)
        .map(|((id, _), g)| EdgeSite {
            cdn: cdn::CDNS.iter().find(|c| c.id == *id).map_or("", |c| c.id),
            site: g.site,
            site_city: None,
            ip: g.ip,
            asn: None,
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
    RollCall { you, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastly_edge_is_the_last_entry() {
        let h = "cache-iad-khef600071-IAD, cache-iad-khef600051-IAD, cache-dfw-kdfw8210059-DFW";
        assert_eq!(fastly_site(h).as_deref(), Some("DFW"));
        assert_eq!(fastly_site("cache-iad-khef600083-IAD, cache-iad-khef600051-IAD, cache-mfe400027-MFE").as_deref(), Some("MFE"));
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
        let body = "203.0.113.124 => lga25s86 (203.0.113.0/24)\n\nDebug Info:\no-ExampleDebugInfoToken\nExampleTok";
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
        let y = cloudflare_you(&headers(&[("asn", "64500"), ("city", "Charlotte"), ("latitude", "35.22709"), ("longitude", "-80.84313")]));
        assert_eq!((y.asn, y.city.as_deref(), y.lat, y.lon), (Some(64500), Some("Charlotte"), Some(35.2), Some(-80.8)));
        let y = cloudflare_you(&headers(&[
            ("cf-meta-asn", "7018"),
            ("cf-meta-city", "Dallas"),
            ("cf-meta-latitude", "32.84"),
            ("cf-meta-longitude", "-96.95"),
        ]));
        assert_eq!((y.asn, y.city.as_deref(), y.lat, y.lon), (Some(7018), Some("Dallas"), Some(32.8), Some(-97.0)));
        let y = cloudflare_you(&headers(&[("asn", "64500"), ("cf-meta-ip", "203.0.113.9")]));
        assert_eq!((y.asn, y.city, y.lat, y.lon), (Some(64500), None, None, None));
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
        println!("you: {:?}", rc.you);
        for e in &rc.edges {
            println!(
                "{:<10} {:<4} {:<12} ip={:?} AS{:?} err={:?}",
                e.cdn,
                e.site.as_deref().unwrap_or("-"),
                e.site_city.as_deref().unwrap_or("-"),
                e.ip,
                e.asn,
                e.error
            );
        }
    }
}
