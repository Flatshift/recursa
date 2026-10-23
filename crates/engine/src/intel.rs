//! Network intelligence from public sources, cached on disk.
//!
//! * Team Cymru DNS ([`crate::cymru`]): origin AS, BGP prefix and AS name for an address.
//! * Route-origin validation on this computer ([`crate::rpki`], opt-in): ROA validity of the Cymru prefix.
//! * RoVista: share of the operator network's routes protected by ROV.
//! * Operator status pages: Cloudflare, Quad9, AdGuard and Cisco Umbrella (OpenDNS).
//! * Threat feeds for filtering probes: URLhaus (malware), OpenPhish (phishing).
//!
//! Nothing here runs while a measured phase is active: callers get the cached copy.

use crate::cymru;
use crate::dns::{self, QuerySpec};
use crate::lock;
use crate::net::{Exchanger, UdpX};
use crate::regions::{self, Client, Places};
use crate::resolve::Resolve;
use crate::store::Store;
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Semaphore;

const HOUR: i64 = 3_600_000;
/// Largest response body we accept from any source.
const MAX_BODY: usize = 16 << 20;

pub struct Intel {
    http: reqwest::Client,
    store: Arc<Store>,
    measuring: Arc<AtomicBool>,
    /// The system resolvers, which carry the Team Cymru questions.
    system: Arc<RwLock<Vec<IpAddr>>>,
    #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
    pub rpki: Arc<crate::rpki::Rpki>,
    /// Bounds concurrent outbound requests so a run's intel phase stays polite.
    permits: Semaphore,
}

#[derive(Default)]
pub struct ThreatFeeds {
    pub malware: Feed,
    pub phishing: Feed,
}

/// One threat feed's usable hosts, and when it was downloaded (Unix ms; none if it never was).
#[derive(Default)]
pub struct Feed {
    pub hosts: Vec<FeedHost>,
    pub fetched_at: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct FeedHost {
    pub host: String,
    /// When the feed first listed this host (Unix ms), for feeds that date their entries.
    pub listed_at: Option<i64>,
}

/// "YYYY-MM-DD HH:MM:SS" in UTC to Unix milliseconds.
fn utc_ms(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let secs = n(11..13).unwrap_or(0) * 3600 + n(14..16).unwrap_or(0) * 60 + n(17..19).unwrap_or(0);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + secs) * 1000)
}

/// Shared-hosting suffixes: a phishing page on one of these doesn't make the whole
/// platform blockable, so such names are useless for testing DNS filters.
const SHARED_HOSTING: &[&str] = &[
    "amazonaws.com",
    "cloudfront.net",
    "github.io",
    "githubusercontent.com",
    "googleapis.com",
    "firebaseapp.com",
    "web.app",
    "pages.dev",
    "workers.dev",
    "vercel.app",
    "netlify.app",
    "blogspot.com",
    "weebly.com",
    "wixsite.com",
    "webflow.io",
    "r2.dev",
    "azurewebsites.net",
    "windows.net",
    "ipfs.io",
    "dweb.link",
    "glitch.me",
    "square.site",
    "google.com",
    "microsoft.com",
    "sharepoint.com",
    "office.com",
    "live.com",
    "dropbox.com",
    "canva.site",
    "framer.app",
    "godaddysites.com",
    "mystrikingly.com",
    "replit.app",
    "onrender.com",
    "herokuapp.com",
    "surge.sh",
];

fn valid_host(h: &str) -> bool {
    h.len() <= 253
        && h.contains('.')
        && h.parse::<IpAddr>().is_err()
        && h.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
        && h.split('.').all(|l| !l.is_empty() && l.len() <= 63)
        && !SHARED_HOSTING.iter().any(|s| h.strip_suffix(s).is_some_and(|rest| rest.is_empty() || rest.ends_with('.')))
}

impl Intel {
    pub fn new(store: Arc<Store>, measuring: Arc<AtomicBool>, system: Arc<RwLock<Vec<IpAddr>>>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Recursa-DNS-Benchmark/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(12))
            .build()
            .expect("http client");
        // Earlier builds downloaded a location database here; remove what they left behind.
        let _ = std::fs::remove_dir_all(store.path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("dbip"));
        #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
        let rpki = crate::rpki::Rpki::new(
            store.path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("rpki"),
            store.clone(),
            measuring.clone(),
        );
        Intel {
            http,
            store,
            measuring,
            system,
            #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
            rpki,
            permits: Semaphore::new(8),
        }
    }

    async fn get_bytes(&self, url: &str) -> Option<bytes::Bytes> {
        let _permit = self.permits.acquire().await.ok()?;
        let mut r = self.http.get(url).send().await.ok()?;
        if !r.status().is_success() || r.content_length().is_some_and(|n| n as usize > MAX_BODY) {
            return None;
        }
        // Read in pieces: a chunked reply has no length to check up front, and must not fill memory first.
        let mut body = bytes::BytesMut::new();
        while let Some(chunk) = r.chunk().await.ok()? {
            if body.len() + chunk.len() > MAX_BODY {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        Some(body.freeze())
    }

    async fn get_json(&self, url: &str) -> Option<Value> {
        serde_json::from_slice(&self.get_bytes(url).await?).ok()
    }

    async fn get_text(&self, url: &str) -> Option<String> {
        String::from_utf8(self.get_bytes(url).await?.to_vec()).ok()
    }

    /// Whether a background download (an RPKI sync) is under way in this process.
    pub fn fetching(&self) -> bool {
        #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
        if self.rpki.syncing() {
            return true;
        }
        false
    }

    /// Fresh cache hit, else fetch (unless measuring), else stale cache.
    async fn cached<F>(&self, key: &str, max_age: i64, fetch: F) -> Option<(Value, i64)>
    where
        F: std::future::Future<Output = Option<Value>>,
    {
        if let Some(hit) = self.store.cache_get(key, max_age) {
            return Some(hit);
        }
        if !self.measuring.load(Ordering::SeqCst)
            && !lock::busy()
            && let Some(v) = fetch.await
        {
            self.store.cache_put(key, &v);
            return Some((v, crate::bench::now_ms()));
        }
        self.store.cache_get_any(key)
    }

    pub async fn threat_feeds(&self) -> ThreatFeeds {
        let malware = self
            .cached("feed:urlhaus-online", HOUR, async {
                // The online-URLs CSV: the hostfile's hosts plus when each URL was added.
                let text = self.get_text("https://urlhaus.abuse.ch/downloads/csv_online/").await?;
                let mut first: std::collections::BTreeMap<String, Option<i64>> = Default::default();
                for line in text.lines().filter(|l| !l.starts_with('#')) {
                    let f: Vec<&str> = line.trim().trim_matches('"').split("\",\"").collect();
                    let (Some(added), Some(url)) = (f.get(1), f.get(2)) else { continue };
                    let Some(host) =
                        url.parse::<http::Uri>().ok().and_then(|u| u.host().map(|h| h.trim_end_matches('.').to_ascii_lowercase()))
                    else {
                        continue;
                    };
                    if valid_host(&host) {
                        let t = utc_ms(added);
                        first
                            .entry(host)
                            .and_modify(|e| {
                                *e = match (*e, t) {
                                    (Some(a), Some(b)) => Some(a.min(b)),
                                    (a, b) => a.or(b),
                                }
                            })
                            .or_insert(t);
                    }
                }
                Some(json!(first.into_iter().collect::<Vec<_>>()))
            })
            .await;
        let phishing = self
            .cached("feed:openphish", HOUR, async {
                let text = self.get_text("https://raw.githubusercontent.com/openphish/public_feed/refs/heads/main/feed.txt").await?;
                let mut hosts: Vec<String> = text
                    .lines()
                    .filter_map(|l| l.trim().parse::<http::Uri>().ok())
                    .filter_map(|u| u.host().map(|h| h.trim_end_matches('.').to_ascii_lowercase()))
                    .filter(|h| valid_host(h))
                    .collect();
                hosts.sort();
                hosts.dedup();
                Some(json!(hosts))
            })
            .await;
        let malware = match malware {
            Some((v, at)) => Feed {
                hosts: serde_json::from_value::<Vec<(String, Option<i64>)>>(v)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(host, listed_at)| FeedHost { host, listed_at })
                    .collect(),
                fetched_at: Some(at),
            },
            None => Feed::default(),
        };
        let phishing = match phishing {
            Some((v, at)) => Feed {
                hosts: serde_json::from_value::<Vec<String>>(v)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|host| FeedHost { host, listed_at: None })
                    .collect(),
                fetched_at: Some(at),
            },
            None => Feed::default(),
        };
        ThreatFeeds { malware, phishing }
    }

    async fn rovista(&self, asn: u64) -> Option<Value> {
        // RoVista's search matches prefixes of the number too, so pick the exact AS.
        let url = format!("https://api.rovista.netsecurelab.org/rovista/api/overview?search={asn}&offset=0&count=20");
        let v = self.get_json(&url).await?;
        let row = v["data"].as_array()?.iter().find(|r| r["asn"].as_u64() == Some(asn))?.clone();
        Some(json!({
            "ratio": row["ratio"],
            "organization": row["organization"],
            "updated": row["lastUpdatedDate"],
        }))
    }

    /// Origin AS and covering prefix from Team Cymru. `None` when the lookup failed; an address no
    /// one announces is `Some((None, None))`.
    async fn origin(&self, ip: IpAddr) -> Option<(Option<u64>, Option<String>)> {
        let res = Resolve::new(self.system.read().unwrap().clone());
        let found = tokio::task::spawn_blocking(move || cymru::origin_of(&res, ip)).await.ok()?.ok()?;
        Some((found.as_ref().map(|o| u64::from(o.asn)), found.map(|o| o.prefix)))
    }

    /// ROA validity of `prefix` announced by AS`asn`, from the local check. Never waits for a sync,
    /// and is not cached: it follows the setting and the data as they are now.
    fn rpki(&self, asn: u64, prefix: &str) -> Value {
        #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
        return self.rpki.lookup(asn, prefix);
        #[cfg(not(all(feature = "rpki-local", not(target_os = "ios"))))]
        {
            let _ = (asn, prefix);
            json!({ "status": "not_checked", "state": "off", "reason": "This build doesn’t check route origins.", "roas": [], "validator": null })
        }
    }

    /// Origin AS, holder, prefix, ROA validity and operator ROV for one address.
    async fn ip_info(&self, ip: IpAddr) -> Value {
        let key = format!("ip:{ip}");
        let got = self
            .cached(&key, 24 * HOUR, async {
                let (asn, prefix) = self.origin(ip).await?;
                let (holder, rov) = match asn {
                    Some(asn) => tokio::join!(self.holder(asn), self.rovista(asn)),
                    None => (Value::Null, None),
                };
                Some(json!({ "ip": ip.to_string(), "asn": asn, "prefix": prefix, "holder": holder, "rov": rov }))
            })
            .await;
        match got {
            Some((mut v, at)) => {
                v["fetched_at"] = json!(at);
                // Added after the cache, so it follows the setting and the latest data; it also replaces
                // what older cache entries held.
                v["rpki"] = match (v["asn"].as_u64(), v["prefix"].as_str()) {
                    (Some(asn), Some(p)) => self.rpki(asn, p),
                    _ => Value::Null,
                };
                v
            }
            None => json!({ "ip": ip.to_string(), "error": "Team Cymru unreachable" }),
        }
    }

    /// Origin AS and holder only: CDN edges and resolver egress don't need RPKI data.
    pub async fn asn_of(&self, ip: IpAddr) -> Value {
        let key = format!("asn:{ip}");
        self.cached(&key, 24 * HOUR, async {
            let (asn, prefix) = self.origin(ip).await?;
            let holder = match asn {
                Some(a) => self.holder(a).await,
                None => Value::Null,
            };
            Some(json!({ "ip": ip.to_string(), "asn": asn, "prefix": prefix, "holder": holder }))
        })
        .await
        .map(|(v, _)| v)
        .unwrap_or(Value::Null)
    }

    async fn holder(&self, asn: u64) -> Value {
        let key = format!("holder:{asn}");
        self.cached(&key, 7 * 24 * HOUR, async {
            let res = Resolve::new(self.system.read().unwrap().clone());
            let name = tokio::task::spawn_blocking(move || cymru::name_of(&res, u32::try_from(asn).ok()?).ok()).await.ok()??;
            Some(name.map_or(Value::Null, |n| json!(cymru::holder_name(&n))))
        })
        .await
        .map(|(v, _)| v)
        .unwrap_or(Value::Null)
    }

    pub async fn resolver_intel(&self, service: Option<IpAddr>, egress: Option<String>) -> Value {
        let service = match service {
            Some(ip) if !crate::catalog::is_private(&ip) => self.ip_info(ip).await,
            Some(ip) => json!({ "ip": ip.to_string(), "private": true }),
            None => Value::Null,
        };
        let egress = match egress.and_then(|e| e.parse::<IpAddr>().ok()) {
            Some(ip) => {
                let mut info = self.asn_of(ip).await;
                if info.is_null() {
                    info = json!({ "ip": ip.to_string() });
                }
                info
            }
            None => Value::Null,
        };
        json!({ "service": service, "egress": egress })
    }

    /// The client's public address and country code from Cloudflare's HTTPS trace endpoint,
    /// which no DNS interception on the path can alter.
    async fn trace_ip(&self, base: &str) -> Option<(IpAddr, Option<String>)> {
        parse_trace(&self.get_text(&format!("{base}/cdn-cgi/trace")).await?)
    }

    /// The client's public addresses plus their networks. HTTPS first; OpenDNS's
    /// myip over plain DNS as a fallback.
    pub async fn vantage(&self) -> Value {
        let (v4, v6) = tokio::join!(self.trace_ip("https://1.1.1.1"), self.trace_ip("https://[2606:4700:4700::1111]"));
        let (v4, v6, country) = match (v4, v6) {
            (None, None) => {
                let (v4, v6) = self.vantage_dns().await;
                (v4, v6, None)
            }
            (v4, v6) => {
                let country = [&v4, &v6].into_iter().find_map(|t| t.as_ref().and_then(|(_, c)| c.clone()));
                (v4.map(|t| t.0), v6.map(|t| t.0), country)
            }
        };
        let mut out = json!({ "ipv4": v4.map(|i| i.to_string()), "ipv6": v6.map(|i| i.to_string()), "ipv6_reachable": v6.is_some() });
        if let Some(ip) = v4.or(v6) {
            out["network"] = self.ip_info(ip).await;
        }
        if let Some(code) = country {
            out["geo"] = json!({ "country": code });
        }
        out
    }

    async fn vantage_dns(&self) -> (Option<IpAddr>, Option<IpAddr>) {
        let ask = |server: &'static str, qtype: u16| {
            tokio::task::spawn_blocking(move || -> Option<IpAddr> {
                let addr: SocketAddr = server.parse().ok()?;
                let mut x = UdpX::connect(addr).ok()?;
                let q = dns::encode_query(dns::new_id(), &QuerySpec::new("myip.opendns.com", qtype)).ok()?;
                let r = x.exchange(&q, Duration::from_millis(1500)).ok()?;
                let m = dns::parse(&r.bytes).ok()?;
                if qtype == dns::TYPE_A {
                    m.ipv4_answers().first().map(|a| IpAddr::V4(*a))
                } else {
                    m.ipv6_answers().first().map(|a| IpAddr::V6(*a))
                }
            })
        };
        let (v4, v6) = tokio::join!(ask("208.67.222.222:53", dns::TYPE_A), ask("[2620:119:35::35]:53", dns::TYPE_AAAA));
        (v4.ok().flatten(), v6.ok().flatten())
    }

    pub async fn status(&self, force: bool) -> Value {
        if force
            && !self.measuring.load(Ordering::SeqCst)
            && !lock::busy()
            && let Some(v) = self.fetch_status().await
        {
            self.store.cache_put("status:all", &v);
        }
        match self.cached("status:all", 10 * 60_000, self.fetch_status()).await {
            Some((v, at)) => json!({ "feeds": v, "fetched_at": at }),
            None => json!({ "feeds": [], "fetched_at": null }),
        }
    }

    async fn fetch_status(&self) -> Option<Value> {
        let (cf_sum, cf_inc, q9, ag_sum, ag_inc, um_sum, um_inc) = tokio::join!(
            self.get_json("https://www.cloudflarestatus.com/api/v2/summary.json"),
            self.get_json("https://www.cloudflarestatus.com/api/v2/incidents.json"),
            self.get_json("https://uptime.quad9.net/index.json"),
            self.get_json("https://status.adguard.com/api/v2/summary.json"),
            self.get_json("https://status.adguard.com/api/v2/incidents.json"),
            self.get_json("https://status.umbrella.cisco.com/api/v2/summary.json"),
            self.get_json("https://status.umbrella.cisco.com/api/v2/incidents.json"),
        );
        let feeds = vec![
            statuspage(
                "cloudflare",
                "Cloudflare",
                "https://www.cloudflarestatus.com",
                cf_sum,
                cf_inc,
                |c, _| c == "Recursive DNS",
                |name| {
                    let n = name.to_ascii_lowercase();
                    n.contains("1.1.1.1") || n.contains("resolver") || n.contains("recursive")
                },
            ),
            better_stack("quad9", "Quad9", "https://uptime.quad9.net", q9),
            statuspage(
                "adguard",
                "AdGuard DNS",
                "https://status.adguard.com",
                ag_sum,
                ag_inc,
                |c, _| c == "AdGuard DNS",
                |name| name.contains("DNS"),
            ),
            statuspage(
                "umbrella",
                "Cisco Umbrella / OpenDNS",
                "https://status.umbrella.cisco.com",
                um_sum,
                um_inc,
                // OpenDNS is Umbrella's own resolver; Secure Connect's DNS is another product.
                |c, group| c == "DNS" && group.starts_with("Umbrella"),
                |name| name.contains("DNS") || name.to_ascii_lowercase().contains("resolution"),
            ),
        ];
        Some(Value::Array(feeds))
    }
}

fn worst(states: impl Iterator<Item = String>) -> String {
    let rank = |s: &str| match s {
        "major_outage" => 5,
        "partial_outage" => 4,
        "degraded_performance" | "degraded" => 3,
        "under_maintenance" | "maintenance" => 2,
        "operational" => 1,
        _ => 0,
    };
    states.max_by_key(|s| rank(s)).unwrap_or_else(|| "unknown".into())
}

/// Incident states that are over.
fn is_closed(status: Option<&str>) -> bool {
    matches!(status, Some("resolved" | "postmortem" | "completed"))
}

/// `{ state, label, regions, countries }`: how bad it is, and where (nothing named: everywhere).
fn scope(state: &str, places: &Places) -> Value {
    let mut v = places.to_json();
    v["state"] = json!(state);
    v
}

fn push_scope(scopes: &mut Vec<Value>, v: Value) {
    if !scopes.contains(&v) {
        scopes.push(v);
    }
}

#[allow(clippy::too_many_arguments)]
fn statuspage(
    feed: &str,
    name: &str,
    page: &str,
    summary: Option<Value>,
    incidents: Option<Value>,
    // (component name, name of its group)
    component: impl Fn(&str, &str) -> bool,
    incident_filter: impl Fn(&str) -> bool,
) -> Value {
    let Some(sum) = summary else {
        return json!({ "feed": feed, "name": name, "page": page, "ok": false, "state": "unknown", "incidents": [] });
    };
    let comps = sum["components"].as_array().cloned().unwrap_or_default();
    let groups: std::collections::HashMap<String, String> = comps
        .iter()
        .filter(|c| c["group"].as_bool() == Some(true))
        .filter_map(|c| Some((c["id"].as_str()?.to_string(), c["name"].as_str()?.to_string())))
        .collect();
    let group_of = |c: &Value| c["group_id"].as_str().and_then(|g| groups.get(g)).cloned();
    let is_ours = |c: &Value| c["name"].as_str().is_some_and(|n| component(n, group_of(c).as_deref().unwrap_or("")));
    let matched: Vec<&Value> = comps.iter().filter(|c| is_ours(c)).collect();
    let state = worst(matched.iter().filter_map(|c| c["status"].as_str().map(str::to_string)));
    let affected: Vec<String> = matched
        .iter()
        .filter(|c| c["status"].as_str() != Some("operational"))
        .map(|c| group_of(c).unwrap_or_else(|| c["name"].as_str().unwrap_or("").to_string()))
        .collect();
    let relevant: Vec<Value> = incidents
        .and_then(|v| v["incidents"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| {
            let n = i["name"].as_str().unwrap_or("");
            incident_filter(n) || i["components"].as_array().is_some_and(|cs| cs.iter().any(&is_ours))
        })
        .collect();
    // Where the trouble is: a regional group says so ("Umbrella Asia"); a service-wide
    // component takes it from the open incidents' own words, else it is everywhere.
    let mut said = Places::default();
    for i in relevant.iter().filter(|i| !is_closed(i["status"].as_str())) {
        let update = i["incident_updates"].as_array().and_then(|u| u.first()).and_then(|u| u["body"].as_str()).unwrap_or("");
        said.merge(regions::detect(&format!("{}. {update}", i["name"].as_str().unwrap_or(""))));
    }
    let mut scopes: Vec<Value> = Vec::new();
    for c in matched.iter().filter(|c| c["status"].as_str().is_some_and(|s| s != "operational")) {
        let by_group = regions::detect(&group_of(c).unwrap_or_default());
        let places = if by_group.is_empty() { &said } else { &by_group };
        push_scope(&mut scopes, scope(c["status"].as_str().unwrap_or("unknown"), places));
    }
    let incidents: Vec<Value> = relevant
        .into_iter()
        .take(12)
        .map(|i| {
            json!({
                "title": i["name"],
                "status": i["status"],
                "started": i["started_at"].as_str().or(i["created_at"].as_str()),
                "resolved": i["resolved_at"],
                // Rendered as a link: only an https URL, else the status page itself.
                "url": i["shortlink"].as_str().filter(|u| u.starts_with("https://")).unwrap_or(page),
                "update": i["incident_updates"].as_array().and_then(|u| u.first()).map(|u| u["body"].clone()),
            })
        })
        .collect();
    json!({
        "feed": feed, "name": name, "page": page, "ok": true,
        "state": if matched.is_empty() { "unknown".to_string() } else { state },
        "component": matched.first().and_then(|c| c["name"].as_str()),
        "affected": affected,
        "scopes": scopes,
        "incidents": incidents,
    })
}

fn better_stack(feed: &str, name: &str, page: &str, v: Option<Value>) -> Value {
    let Some(v) = v else {
        return json!({ "feed": feed, "name": name, "page": page, "ok": false, "state": "unknown", "incidents": [] });
    };
    let state = match v["data"]["attributes"]["aggregate_state"].as_str().unwrap_or("unknown") {
        "operational" => "operational",
        "degraded" => "degraded_performance",
        "downtime" => "major_outage",
        "maintenance" => "under_maintenance",
        _ => "unknown",
    };
    let included = v["included"].as_array().cloned().unwrap_or_default();
    let update = included
        .iter()
        .filter(|i| i["type"] == "status_update")
        .filter_map(|i| i["attributes"]["message"].as_str())
        .next()
        .map(str::to_string);
    // Open reports carry the place in their title ("Service degradation in Asia") or update.
    let mut said = Places::default();
    for r in included.iter().filter(|i| i["type"] == "status_report" && i["attributes"]["ends_at"].is_null()) {
        said.merge(regions::detect(&format!("{}. {}", r["attributes"]["title"].as_str().unwrap_or(""), update.as_deref().unwrap_or(""))));
    }
    let scopes = if matches!(state, "operational" | "unknown") { vec![] } else { vec![scope(state, &said)] };
    let incidents: Vec<Value> = included
        .iter()
        .filter(|i| i["type"] == "status_report")
        .map(|i| {
            let a = &i["attributes"];
            json!({
                "title": a["title"],
                "status": if a["ends_at"].is_null() { "investigating" } else { "resolved" },
                "started": a["starts_at"],
                "resolved": a["ends_at"],
                "url": page,
                "update": update,
            })
        })
        .collect();
    json!({ "feed": feed, "name": name, "page": page, "ok": true, "state": state, "component": "All services", "affected": [], "scopes": scopes, "incidents": incidents })
}

/// Settles each feed's state for this client. Operators flag a whole service when one
/// region has trouble, so a report that names only places away from the client leaves the
/// resolver operational here; `elsewhere` says what and where, so nothing is hidden. A
/// report naming no place reaches everyone, and with no known location nothing is cleared.
/// `global_state` keeps the operator's own overall word.
pub fn localize(feeds: &mut Value, client: Option<&Client>) {
    for f in feeds.as_array_mut().into_iter().flatten() {
        f["global_state"] = f["state"].clone();
        if let Some(c) = client {
            f["client"] = json!({ "code": c.code, "country": c.name });
        }
        let scopes = f["scopes"].as_array().cloned().unwrap_or_default();
        if scopes.is_empty() {
            continue;
        }
        let (mut here, mut away, mut where_) = (Vec::new(), Vec::new(), Vec::new());
        let mut everywhere = false;
        for s in &scopes {
            let places = Places::from_json(s);
            let state = s["state"].as_str().unwrap_or("unknown").to_string();
            match client {
                _ if places.is_empty() => {
                    everywhere = true;
                    here.push(state);
                }
                Some(c) if !places.covers(c) => away.push(places.label()),
                _ => {
                    where_.push(places.label());
                    here.push(state);
                }
            }
        }
        away.dedup();
        where_.dedup();
        let located = client.is_some() || everywhere;
        f["locality"] = json!(match (here.is_empty(), everywhere, client) {
            (true, ..) => "elsewhere",
            (false, true, _) => "global",
            (false, false, Some(_)) => "local",
            (false, false, None) => "unknown",
        });
        if !located {
            // Reports with places, and nowhere to compare them with: say so, change nothing.
            f["where"] = json!(where_.join("; "));
            continue;
        }
        f["state"] = json!(if here.is_empty() { "operational".to_string() } else { worst(here.into_iter()) });
        f["where"] = json!(where_.join("; "));
        f["elsewhere"] = json!(away);
    }
}

/// The address and two-letter country code from a Cloudflare trace body. `XX` (unknown),
/// `T1` (Tor) and anything else that is not two ASCII letters count as no country.
fn parse_trace(body: &str) -> Option<(IpAddr, Option<String>)> {
    let field = |k: &str| body.lines().find_map(|l| l.strip_prefix(k)).map(str::trim);
    let ip = field("ip=")?.parse().ok()?;
    let country = field("loc=")
        .filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic()) && !c.eq_ignore_ascii_case("XX"))
        .map(str::to_ascii_uppercase);
    Some((ip, country))
}

pub fn curated_incidents() -> Value {
    static PARSED: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(include_str!("../../../data/incidents.json")).unwrap_or(Value::Array(vec![]))).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlhaus_dates_convert_to_unix_ms() {
        assert_eq!(utc_ms("1970-01-01 00:00:00"), Some(0));
        assert_eq!(utc_ms("2000-03-01 00:00:00"), Some(951_868_800_000));
        assert_eq!(utc_ms("2025-07-14 12:34:56"), Some(1_752_496_496_000));
        assert_eq!(utc_ms("dateadded"), None);
    }

    #[test]
    fn trace_gives_address_and_country() {
        let (ip, loc) = parse_trace("fl=1\nip=203.0.113.9\nts=1\nloc=de\n").unwrap();
        assert_eq!(ip.to_string(), "203.0.113.9");
        assert_eq!(loc.as_deref(), Some("DE"));
        for body in ["ip=2001:db8::1\nloc=XX\n", "ip=2001:db8::1\nloc=T1\n", "ip=2001:db8::1\n"] {
            assert_eq!(parse_trace(body).unwrap().1, None);
        }
        assert!(parse_trace("loc=US\n").is_none());
    }

    #[test]
    fn shared_hosting_is_excluded_by_whole_labels() {
        assert!(!valid_host("github.io"));
        assert!(!valid_host("phish.github.io"));
        assert!(valid_host("notgithub.io"));
        assert!(valid_host("evil-login.example"));
        assert!(!valid_host("192.0.2.1"));
    }

    fn quad9_notice(title: &str, text: &str) -> Value {
        better_stack(
            "quad9",
            "Quad9",
            "https://uptime.quad9.net",
            Some(json!({
                "data": { "attributes": { "aggregate_state": "degraded" } },
                "included": [
                    { "type": "status_report", "attributes": { "title": title, "starts_at": "2026-09-23T11:04:00.000Z", "ends_at": null, "aggregate_state": "degraded" } },
                    { "type": "status_update", "attributes": { "message": text } },
                ],
            })),
        )
    }

    fn settled(feed: Value, country: Option<&str>) -> Value {
        let mut feeds = json!([feed]);
        localize(&mut feeds, country.and_then(Client::from_code).as_ref());
        feeds[0].clone()
    }

    #[test]
    fn a_report_for_another_region_leaves_the_resolver_operational_here() {
        let feed = quad9_notice(
            "Service degradation in Asia",
            "Reduced network capacity in Asia from subsea cable outages, mostly traffic through Singapore, Hong Kong and Taiwan.",
        );
        assert_eq!(feed["state"], "degraded_performance");
        let here = settled(feed.clone(), Some("US"));
        assert_eq!((here["state"].as_str(), here["locality"].as_str()), (Some("operational"), Some("elsewhere")));
        assert_eq!(here["global_state"], "degraded_performance");
        assert!(here["elsewhere"][0].as_str().unwrap().contains("Asia"));
        assert_eq!(here["client"]["country"], "United States");
        // Where it applies, it still reads as degraded, and says where.
        let there = settled(feed.clone(), Some("JP"));
        assert_eq!((there["state"].as_str(), there["locality"].as_str()), (Some("degraded_performance"), Some("local")));
        assert!(there["where"].as_str().unwrap().contains("Asia"));
        // Without a location nothing is cleared.
        let lost = settled(feed, None);
        assert_eq!((lost["state"].as_str(), lost["locality"].as_str()), (Some("degraded_performance"), Some("unknown")));
    }

    #[test]
    fn a_report_that_names_no_place_reaches_everyone() {
        let feed = quad9_notice("Investigating resolver errors", "Some queries are failing.");
        let f = settled(feed, Some("US"));
        assert_eq!((f["state"].as_str(), f["locality"].as_str()), (Some("degraded_performance"), Some("global")));
        // And an operational feed has nothing to settle.
        let ok = settled(json!({ "feed": "x", "ok": true, "state": "operational", "scopes": [] }), Some("US"));
        assert_eq!(ok["state"], "operational");
        assert!(ok["locality"].is_null());
    }

    #[test]
    fn regional_groups_say_where_a_component_is_degraded() {
        let comp = |id: &str, name: &str, group: Option<&str>, status: &str| json!({ "id": id, "name": name, "group": group.is_none(), "group_id": group, "status": status });
        let summary = json!({ "components": [
            comp("g1", "Umbrella Asia", None, "operational"),
            comp("g2", "Umbrella North America", None, "operational"),
            comp("g3", "Secure Connect North America", None, "operational"),
            comp("c1", "DNS", Some("g1"), "degraded_performance"),
            comp("c2", "DNS", Some("g2"), "operational"),
            // A different product: it must not count against OpenDNS.
            comp("c3", "DNS", Some("g3"), "major_outage"),
        ]});
        let feed = statuspage(
            "umbrella",
            "Umbrella",
            "https://status.umbrella.cisco.com",
            Some(summary),
            None,
            |c, group| c == "DNS" && group.starts_with("Umbrella"),
            |n| n.contains("DNS"),
        );
        assert_eq!(feed["state"], "degraded_performance");
        let us = settled(feed.clone(), Some("US"));
        assert_eq!((us["state"].as_str(), us["locality"].as_str()), (Some("operational"), Some("elsewhere")));
        let sg = settled(feed, Some("SG"));
        assert_eq!(sg["state"], "degraded_performance");
    }

    #[test]
    fn a_service_wide_component_takes_its_place_from_the_open_incident() {
        let summary = json!({ "components": [{ "id": "c", "name": "Recursive DNS", "status": "partial_outage" }] });
        let incidents = json!({ "incidents": [
            { "name": "1.1.1.1 resolver errors in Europe", "status": "investigating", "components": [],
              "incident_updates": [{ "body": "Resolvers in Frankfurt and Amsterdam are failing." }] },
            { "name": "1.1.1.1 resolver errors in North America", "status": "resolved", "components": [],
              "incident_updates": [{ "body": "Resolved." }] },
        ]});
        let feed = statuspage(
            "cloudflare",
            "Cloudflare",
            "https://www.cloudflarestatus.com",
            Some(summary),
            Some(incidents),
            |c, _| c == "Recursive DNS",
            |n| n.contains("resolver"),
        );
        // The resolved North America incident doesn't count; the open one names Europe.
        assert_eq!(settled(feed.clone(), Some("US"))["state"], "operational");
        assert_eq!(settled(feed, Some("DE"))["state"], "partial_outage");
    }
}
