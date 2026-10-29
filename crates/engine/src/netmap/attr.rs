//! Which network each router belongs to, and how the traces fit into one picture.
//!
//! Hops are attributed here, on this computer, by longest-prefix match against CAIDA's prefix-to-AS table (fetched
//! once a day, never bundled). No hop address is sent anywhere: [`attribute`] and [`group`] take the table and
//! PeeringDB already loaded and make no request.

use super::asrel::{AsRel, Rel};
use super::pdb::Pdb;
use super::rdap::Block;
use crate::catalog::is_private;
use crate::stats::median;
use crate::store::Store;
use crate::trace::Trace;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

const LOG_URL: &str = "https://publicdata.caida.org/datasets/routing/routeviews-prefix2as/pfx2as-creation.log";
const DIR_URL: &str = "https://publicdata.caida.org/datasets/routing/routeviews-prefix2as/";
const LOG6_URL: &str = "https://publicdata.caida.org/datasets/routing/routeviews6-prefix2as/pfx2as-creation.log";
const DIR6_URL: &str = "https://publicdata.caida.org/datasets/routing/routeviews6-prefix2as/";
pub(crate) const FETCHED_AT: &str = "netmap.pfx2as_fetched_at";
pub(crate) const SOURCE: &str = "netmap.pfx2as_source";
pub(crate) const SOURCE6: &str = "netmap.pfx2as6_source";
/// Shortest prefix tried, as announced to the Internet's default-free zone.
const SHORTEST: u8 = 8;
/// Shortest IPv6 prefix tried.
const SHORTEST6: u8 = 16;

// ---------------------------------------------------------------------------------------------
// The prefix-to-AS table
// ---------------------------------------------------------------------------------------------

/// What a prefix lookup found.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub asn: u32,
    /// Several networks announce this prefix; `asn` is the first one listed.
    pub moas: bool,
    pub prefix: String,
}

/// Prefixes to their origin, probed by (length, masked address) from the longest down; one map per family.
#[derive(Default)]
pub struct Pfx2as {
    map: HashMap<(u8, u32), (u32, bool)>,
    map6: HashMap<(u8, u128), (u32, bool)>,
}

fn mask(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - len) }
}

fn mask6(len: u8) -> u128 {
    if len == 0 { 0 } else { u128::MAX << (128 - len) }
}

impl Pfx2as {
    pub fn from_rows(rows: impl Iterator<Item = (u32, u8, u32, bool)>, rows6: impl Iterator<Item = (u128, u8, u32, bool)>) -> Pfx2as {
        Pfx2as {
            map: rows.map(|(prefix, len, asn, moas)| ((len, prefix & mask(len)), (asn, moas))).collect(),
            map6: rows6.map(|(prefix, len, asn, moas)| ((len, prefix & mask6(len)), (asn, moas))).collect(),
        }
    }

    pub fn load(store: &Store) -> Pfx2as {
        let mut map = HashMap::new();
        store.pfx2as_each(|prefix, len, asn, moas| {
            map.insert((len, prefix), (asn, moas));
        });
        let mut map6 = HashMap::new();
        store.pfx2as6_each(|prefix, len, asn, moas| {
            map6.insert((len, prefix), (asn, moas));
        });
        Pfx2as { map, map6 }
    }

    /// The IPv4 table is empty. Databases from before the IPv6 table have none until the next refresh, and an IPv6
    /// hop is then unknown.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The longest announced prefix holding `addr`. Private, shared (carrier-grade NAT) and link-local space never
    /// matches.
    pub fn lookup(&self, addr: IpAddr) -> Option<Match> {
        let addr = addr.to_canonical();
        if is_private(&addr) {
            return None;
        }
        match addr {
            IpAddr::V4(a) => {
                let a = u32::from(a);
                (SHORTEST..=32).rev().find_map(|len| {
                    let net = a & mask(len);
                    let &(asn, moas) = self.map.get(&(len, net))?;
                    Some(Match { asn, moas, prefix: format!("{}/{len}", Ipv4Addr::from(net)) })
                })
            }
            IpAddr::V6(a) => {
                let a = u128::from(a);
                (SHORTEST6..=128).rev().find_map(|len| {
                    let net = a & mask6(len);
                    let &(asn, moas) = self.map6.get(&(len, net))?;
                    Some(Match { asn, moas, prefix: format!("{}/{len}", Ipv6Addr::from(net)) })
                })
            }
        }
    }

    pub fn asn_of(&self, addr: IpAddr) -> Option<u32> {
        self.lookup(addr).map(|m| m.asn)
    }
}

/// One row of either table: masked network, length, origin, whether several networks announce it.
#[derive(Debug, PartialEq)]
enum Row {
    V4(u32, u8, u32, bool),
    V6(u128, u8, u32, bool),
}

/// One line of the table: `prefix<TAB>length<TAB>origin`, where the origin is `asn`, or `a_b` or `a,b` when several
/// networks announce it (the first is kept and the row flagged). The IPv4 and IPv6 files share the format.
fn parse_line(line: &str) -> Option<Row> {
    let mut f = line.split('\t');
    let prefix: IpAddr = f.next()?.parse().ok()?;
    let max = if prefix.is_ipv4() { 32 } else { 128 };
    let len: u8 = f.next()?.parse().ok().filter(|l| *l <= max)?;
    let origin = f.next()?.trim();
    let asn = origin.split(['_', ',']).next()?.parse().ok()?;
    let moas = origin.contains(['_', ',']);
    Some(match prefix {
        IpAddr::V4(a) => Row::V4(u32::from(a) & mask(len), len, asn, moas),
        IpAddr::V6(a) => Row::V6(u128::from(a) & mask6(len), len, asn, moas),
    })
}

/// The path of the newest file in the creation log: its last line, `seqnum<TAB>timestamp<TAB>path`.
fn newest(log: &str) -> Option<&str> {
    log.lines().rev().find(|l| !l.starts_with('#') && !l.trim().is_empty())?.split('\t').nth(2).map(str::trim)
}

/// Fetch the newest IPv4 and IPv6 tables and replace the stored ones. `fetch` gets a URL and returns the body (it is
/// where the caller refuses to fetch while a measurement runs). Each table is replaced as soon as its own download
/// parses, so a failed IPv6 fetch leaves the fresh IPv4 table; an old table stays if its own fetch fails.
/// `FETCHED_AT` is written only after both.
pub async fn refresh<F, Fut>(store: &Arc<Store>, fetch: F) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let path = fetch_table(store, &fetch, LOG_URL, DIR_URL, false).await?;
    store.put(SOURCE, &path);
    let path6 = fetch_table(store, &fetch, LOG6_URL, DIR6_URL, true).await?;
    store.put(SOURCE6, &path6);
    store.put(FETCHED_AT, &crate::bench::now_ms());
    Ok(())
}

/// Fetch the newest file of one table's log and store its rows; returns the file's name.
async fn fetch_table<F, Fut>(store: &Arc<Store>, fetch: &F, log_url: &str, dir_url: &str, v6: bool) -> Result<String, String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let log = String::from_utf8(fetch(log_url.into()).await?).map_err(|e| e.to_string())?;
    let path = newest(&log).ok_or("CAIDA's file list has no entries.")?.to_string();
    let gz = fetch(format!("{dir_url}{path}")).await?;
    let s = store.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let lines = BufReader::new(GzDecoder::new(&gz[..]).take(super::BODY_MAX as u64)).lines();
        let rows = lines.map_while(Result::ok).filter_map(|l| parse_line(&l));
        if v6 {
            s.pfx2as6_replace(rows.filter_map(|r| if let Row::V6(p, l, a, m) = r { Some((p, l, a, m)) } else { None }))
        } else {
            s.pfx2as_replace(rows.filter_map(|r| if let Row::V4(p, l, a, m) = r { Some((p, l, a, m)) } else { None }))
        }
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if rows == 0 {
        return Err("CAIDA's table had no rows.".into());
    }
    Ok(path.rsplit('/').next().unwrap_or(&path).to_string())
}

// ---------------------------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------------------------

/// What a group is to the person reading the map; the page colours by it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Private addresses before the first public router.
    Home,
    /// The network you pay.
    Isp,
    /// Any other network on the way.
    Carrier,
    /// An exchange's peering LAN.
    Exchange,
    /// The network the target is in.
    Content,
    /// A public address no announced prefix holds.
    Unknown,
    /// Hops that never answered.
    Silent,
}

/// One router address seen at one hop.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Member {
    pub ttl: u8,
    pub addr: IpAddr,
    pub best_ms: Option<f64>,
    pub sent: u32,
    pub recv: u32,
    /// The announced prefix that holds the address, from the table.
    pub prefix: Option<String>,
    pub moas: bool,
    /// The reverse name, only when decoding router names is on.
    pub name: Option<String>,
    /// A city the name points to, only when the round trip leaves room for it.
    pub hint: Option<String>,
}

/// A run of one trace's hops in the same network.
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub role: Role,
    pub asn: Option<u32>,
    pub holder: Option<String>,
    pub ix: Option<(i64, String)>,
    pub members: Vec<Member>,
    pub ttl_from: u8,
    pub ttl_to: u8,
    /// Private hops inside a public network, named for the networks on both sides.
    pub unnumbered: bool,
    /// For an unknown network: the registry's record for the block its first router is in, when it was looked up.
    pub registry: Option<Block>,
}

/// What attribution needs besides the trace.
pub struct Ctx<'a> {
    pub pfx: &'a Pfx2as,
    pub pdb: &'a Pdb,
    /// Your network.
    pub home: Option<u32>,
    /// Reverse names by address; empty unless decoding is on.
    pub names: &'a HashMap<IpAddr, String>,
    /// Where the geo-IP puts you, for the fibre-floor check on city hints.
    pub you: Option<(f64, f64)>,
    /// Registry records by router address, for addresses no network announces; empty unless the user allowed it.
    pub registry: &'a HashMap<IpAddr, Block>,
    /// Network names by number, for a destination that never answered: no router of its own to take the name from.
    pub holders: &'a HashMap<u32, String>,
}

/// Set each public hop's `asn` from its first address. A local lookup: nothing is sent.
pub fn attribute(trace: &mut Trace, pfx: &Pfx2as) {
    for h in trace.hops.iter_mut().filter(|h| !h.private) {
        h.asn = h.addrs.first().and_then(|a| pfx.asn_of(*a));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Home,
    Net(u32),
    Ix(i64),
    Unknown,
    Silent,
    /// Private, after a public hop: decided from its neighbours.
    Pending,
}

/// The exchange an address is on: its port's, else its LAN's.
fn ix_of(pdb: &Pdb, a: IpAddr) -> Option<i64> {
    pdb.port_of(a).map(|p| p.ix_id).or_else(|| pdb.lan_of(a).map(|l| l.ix_id))
}

/// Collapse a trace into runs of consecutive hops in the same network. Private hops before the first public one are
/// your home. Later private hops are unnumbered and inside a network only when the hops on both sides are in that same
/// network, else unknown. Only neighbours merge: a path that leaves a network and comes back shows both visits, and
/// a trace's first and last hops keep their place in the sequence.
pub fn group(trace: &Trace, cx: &Ctx) -> Vec<Seg> {
    let dest = cx.pfx.asn_of(trace.target);
    let mut seen_public = false;
    let mut keys: Vec<Key> = trace
        .hops
        .iter()
        .map(|h| match h.addrs.first() {
            None => Key::Silent,
            Some(a) if is_private(a) => {
                if seen_public {
                    Key::Pending
                } else {
                    Key::Home
                }
            }
            Some(a) => {
                seen_public = true;
                match (ix_of(cx.pdb, *a), cx.pfx.asn_of(*a)) {
                    (Some(id), _) => Key::Ix(id),
                    (None, Some(asn)) => Key::Net(asn),
                    (None, None) => Key::Unknown,
                }
            }
        })
        .collect();
    let mut unnumbered = vec![false; keys.len()];
    let mut i = 0;
    while i < keys.len() {
        let kind = keys[i];
        if !matches!(kind, Key::Pending | Key::Silent) {
            i += 1;
            continue;
        }
        let end = (i..keys.len()).find(|&j| keys[j] != kind).unwrap_or(keys.len());
        let inside = match (keys.get(i.wrapping_sub(1)), keys.get(end)) {
            (Some(a @ Key::Net(_)), Some(b)) if a == b => Some(*a),
            _ => None,
        };
        for j in i..end {
            if kind == Key::Pending {
                keys[j] = inside.unwrap_or(Key::Unknown);
                unnumbered[j] = inside.is_some();
            } else if let Some(k) = inside {
                // Routers of one network that don't answer are still that network.
                keys[j] = k;
            }
        }
        i = end;
    }
    // Silence before the first answer says nothing: a home router that drops the first probe is still your home.
    let lead = keys.iter().take_while(|k| **k == Key::Silent).count();
    let lead = if lead == keys.len() { 0 } else { lead };
    let mut segs: Vec<Seg> = Vec::new();
    let mut last: Option<Key> = None;
    for (n, h) in trace.hops.iter().enumerate().skip(lead) {
        let key = keys[n];
        if last != Some(key) {
            let (role, asn, ix) = match key {
                Key::Home => (Role::Home, None, None),
                Key::Net(a) if Some(a) == cx.home => (Role::Isp, Some(a), None),
                Key::Net(a) if Some(a) == dest => (Role::Content, Some(a), None),
                Key::Net(a) => (Role::Carrier, Some(a), None),
                Key::Ix(id) => (Role::Exchange, None, Some((id, cx.pdb.ix(id).map(|x| x.name.clone()).unwrap_or_default()))),
                Key::Unknown | Key::Pending => (Role::Unknown, None, None),
                Key::Silent => (Role::Silent, None, None),
            };
            segs.push(Seg {
                role,
                asn,
                holder: None,
                ix,
                members: Vec::new(),
                ttl_from: h.ttl,
                ttl_to: h.ttl,
                unnumbered: false,
                registry: None,
            });
            last = Some(key);
        }
        let seg = segs.last_mut().unwrap();
        seg.ttl_to = h.ttl;
        seg.unnumbered |= unnumbered[n];
        if seg.holder.is_none() && !matches!(key, Key::Ix(_)) {
            seg.holder = h.holder.clone();
        }
        if seg.role == Role::Unknown && seg.registry.is_none() {
            seg.registry = h.addrs.iter().find_map(|a| cx.registry.get(a).cloned());
            seg.holder = seg.registry.as_ref().map(|b| b.holder().to_string());
        }
        for a in &h.addrs {
            let found = cx.pfx.lookup(*a);
            let name = cx.names.get(a).cloned();
            let hint = name.as_deref().and_then(|n| super::rdns::hint(n, h.best_ms, cx.you));
            seg.members.push(Member {
                ttl: h.ttl,
                addr: *a,
                best_ms: h.best_ms,
                sent: h.sent,
                recv: h.recv,
                moas: found.as_ref().is_some_and(|m| m.moas),
                prefix: found.map(|m| m.prefix),
                name,
                hint,
            });
        }
    }
    // A destination that never answered is still where the trace was going: a box with no routers after the last
    // network that did, so the path ends where it was meant to and not at whoever answered last.
    if !trace.reached
        && (dest.is_none() || dest != cx.home)
        && let Some(last) = segs.last()
        && last.role != Role::Content
    {
        let ttl = last.ttl_to.saturating_add(1);
        let holder = dest.and_then(|a| {
            cx.holders.get(&a).cloned().or_else(|| trace.hops.iter().find(|h| h.asn == Some(a)).and_then(|h| h.holder.clone()))
        });
        segs.push(Seg {
            role: Role::Content,
            asn: dest,
            holder,
            ix: None,
            members: Vec::new(),
            ttl_from: ttl,
            ttl_to: ttl,
            unnumbered: false,
            registry: None,
        });
    }
    segs
}

// ---------------------------------------------------------------------------------------------
// The merged picture
// ---------------------------------------------------------------------------------------------

/// A network (or exchange, or stretch of silence) at one place in the picture, shared by every trace that goes through it.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Group {
    pub id: usize,
    pub role: Role,
    pub label: String,
    pub asn: Option<u32>,
    pub holder: Option<String>,
    /// Who the network's number is registered to, when that is a different organisation from the name it goes by:
    /// a network keeps its name after it is bought.
    pub owner: Option<String>,
    pub ix_id: Option<i64>,
    pub ix_name: Option<String>,
    pub ttl_from: u8,
    pub ttl_to: u8,
    /// Traces reached this group through different routers at the same hop: one group, not two branches.
    pub parallel: bool,
    pub unnumbered: bool,
    /// A city the router names point to and the round trips allow: "consistent with", never "in".
    pub city_hint: Option<String>,
    /// Median over the traces of the best round trip to the first router here that answered.
    pub rtt_ms: Option<f64>,
    pub members: Vec<Member>,
    /// Addresses that are possibly the same router: a /30 or /31 pair, or one reverse name.
    pub aliases: Vec<Vec<IpAddr>>,
    /// The ids of the targets whose traces go through here.
    pub targets: Vec<String>,
}

/// How a link got the traffic across.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkKind {
    /// Straight to the network the target is in.
    Direct,
    /// Onto an exchange's peering LAN.
    Exchange {
        ix_id: i64,
    },
    /// To a network that carries it on.
    ViaTransit,
    Unknown,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Link {
    pub from: usize,
    pub to: usize,
    /// Traces that cross it.
    pub count: u32,
    /// The ids of the targets whose traces cross it.
    pub targets: Vec<String>,
    /// Median over the traces of the round trip to the far side's first router minus the round trip to the near
    /// side's first router: what crossing the near network and the hand-off adds, so the steps along a path add up.
    pub rtt_step_ms: Option<f64>,
    #[serde(flatten)]
    pub kind: LinkKind,
    /// What CAIDA's AS relationships say the near network is to the far one; absent when the pair is not listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel: Option<Rel>,
    /// The link that leaves your ISP.
    pub handoff: bool,
    /// The most routers between the two sides that did not answer on one trace: the hand-off is at one of them or at
    /// the far side's first router. Zero when the far side answered right after the near side on every trace.
    pub silent_hops: u8,
}

/// Whether `owner` is another organisation than `holder` names, not the same one written differently: letters and
/// digits only are compared, and one containing the other (`Zayo` in `Zayo Bandwidth`) counts as the same.
pub fn distinct_owner(holder: Option<&str>, owner: &str) -> bool {
    let plain = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect::<String>();
    let o = plain(owner);
    match holder.map(short_holder).map(plain) {
        Some(h) => !o.is_empty() && !h.contains(&o) && !o.contains(&h),
        None => !o.is_empty(),
    }
}

/// A holder as the map names it: the registry's handle goes (`LEVEL3 - Level 3 Parent, LLC` reads `Level 3 Parent, LLC`).
pub fn short_holder(holder: &str) -> &str {
    holder.split_once(" - ").map_or(holder, |(_, rest)| rest)
}

fn label(role: Role, asn: Option<u32>, holder: Option<&str>, ix: Option<&str>) -> String {
    match role {
        Role::Home => "Your home".into(),
        Role::Silent => "No reply".into(),
        Role::Exchange => ix.filter(|n| !n.is_empty()).unwrap_or("Exchange").into(),
        Role::Unknown => holder.map_or_else(|| "Unknown network".into(), |h| format!("{h} (registry)")),
        _ => holder.map(short_holder).map(str::to_string).or(asn.map(|a| format!("AS{a}"))).unwrap_or_else(|| "Unknown network".into()),
    }
}

/// What a route crosses, for comparing two routes: the networks and exchanges in order, repeats collapsed. Home,
/// silent and unknown segments are left out, so a gap in the prefix table does not make a route look different.
pub fn route_key(segs: &[Seg]) -> Vec<(Role, Option<u32>, Option<i64>)> {
    let mut key: Vec<(Role, Option<u32>, Option<i64>)> = Vec::new();
    for s in segs.iter().filter(|s| s.asn.is_some() || s.ix.is_some()) {
        let k = (s.role, s.asn, s.ix.as_ref().map(|(id, _)| *id));
        if key.last() != Some(&k) {
            key.push(k);
        }
    }
    key
}

/// Two addresses at the two ends of one point-to-point link: the same /30 or /31.
pub fn same_link(a: IpAddr, b: IpAddr) -> bool {
    match (a.to_canonical(), b.to_canonical()) {
        (IpAddr::V4(a), IpAddr::V4(b)) if a != b => {
            let (a, b) = (u32::from(a), u32::from(b));
            a >> 2 == b >> 2 || a >> 1 == b >> 1
        }
        _ => false,
    }
}

fn aliases(members: &[Member]) -> Vec<Vec<IpAddr>> {
    let mut cluster: Vec<usize> = (0..members.len()).collect();
    for i in 0..members.len() {
        for j in 0..i {
            let (a, b) = (&members[i], &members[j]);
            if same_link(a.addr, b.addr) || (a.name.is_some() && a.name == b.name && a.addr != b.addr) {
                let (from, to) = (cluster[i], cluster[j]);
                cluster.iter_mut().filter(|c| **c == from).for_each(|c| *c = to);
            }
        }
    }
    let mut out: Vec<Vec<IpAddr>> = Vec::new();
    for root in 0..members.len() {
        let all: Vec<IpAddr> = (0..members.len()).filter(|&i| cluster[i] == root).map(|i| members[i].addr).collect();
        if all.len() > 1 {
            out.push(all);
        }
    }
    out
}

/// Join the traces' runs into one picture. A named network or exchange is one group however it is reached, carrier
/// or destination, unless a
/// trace comes back to it after leaving (a second visit) or its routers decode to another city; silence between two
/// groups that answer goes on the link between them as `silent_hops`, and silence at the end of a trace or unknown
/// addresses are only the same group when they follow the same group. When the routers differ at one hop on the
/// same side the group is marked `parallel`. Returns the groups, the links between them, and for each input the ids
/// of its groups in order. `rels` sets each link's kind where it knows the pair; a link out of an exchange is asked
/// about the last named network before the exchange on the trace that crossed it.
pub fn merge(paths: &[(&str, Vec<Seg>)], rels: &AsRel) -> (Vec<Group>, Vec<Link>, Vec<Vec<usize>>) {
    let mut groups: Vec<Group> = Vec::new();
    let mut before: Vec<Option<usize>> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut steps: Vec<Vec<f64>> = Vec::new();
    let mut arrive: Vec<Vec<f64>> = Vec::new();
    let mut ids: Vec<Vec<usize>> = Vec::new();
    // For a link out of an exchange: the network that fed the exchange.
    let mut fed: Vec<Option<u32>> = Vec::new();
    for (target, segs) in paths {
        let mut last_named: Option<u32> = None;
        let mut prev: Option<(usize, &Seg)> = None;
        let mut mine = Vec::new();
        let mut silence: u8 = 0;
        for (n, seg) in segs.iter().enumerate() {
            // Silence between two groups that answer is drawn on the link between them, not as a place of its own: the
            // hand-off is at one of those routers or the next, and nothing says which network they are in.
            if seg.role == Role::Silent
                && n + 1 < segs.len()
                && let Some((from, _)) = prev
            {
                silence = seg.ttl_to - seg.ttl_from + 1;
                mine.push(from);
                continue;
            }
            let ix_id = seg.ix.as_ref().map(|(id, _)| *id);
            let side = prev.map(|p| p.0);
            let hint = seg.members.iter().find_map(|m| m.hint.as_deref());
            let id = match groups.iter().position(|g| {
                // A network that carries one trace and is another's destination is still one network.
                let same_role =
                    g.role == seg.role || matches!((g.role, seg.role), (Role::Carrier, Role::Content) | (Role::Content, Role::Carrier));
                same_role
                    && g.asn == seg.asn
                    && g.ix_id == ix_id
                    && match seg.role {
                        Role::Isp | Role::Carrier | Role::Content | Role::Exchange => {
                            !mine.contains(&g.id)
                                && match (hint, g.members.iter().find_map(|m| m.hint.as_deref())) {
                                    (Some(a), Some(b)) => a == b,
                                    _ => true,
                                }
                        }
                        _ => before[g.id] == side && (seg.role != Role::Unknown || g.holder == seg.holder),
                    }
            }) {
                Some(id) => {
                    let g = &mut groups[id];
                    for m in &seg.members {
                        match g.members.iter_mut().find(|x| x.addr == m.addr) {
                            Some(x) => x.best_ms = [x.best_ms, m.best_ms].into_iter().flatten().reduce(f64::min),
                            None => {
                                g.parallel |= before[id] == side && g.members.iter().any(|x| x.ttl == m.ttl);
                                g.members.push(m.clone());
                            }
                        }
                    }
                    g.ttl_from = g.ttl_from.min(seg.ttl_from);
                    g.ttl_to = g.ttl_to.max(seg.ttl_to);
                    g.unnumbered &= seg.unnumbered;
                    if g.holder.is_none() {
                        g.holder = seg.holder.clone();
                    }
                    if seg.role == Role::Content {
                        g.role = Role::Content;
                    }
                    id
                }
                None => {
                    let id = groups.len();
                    groups.push(Group {
                        id,
                        role: seg.role,
                        label: String::new(),
                        asn: seg.asn,
                        holder: seg.holder.clone(),
                        owner: None,
                        ix_id,
                        ix_name: seg.ix.as_ref().map(|(_, n)| n.clone()),
                        ttl_from: seg.ttl_from,
                        ttl_to: seg.ttl_to,
                        parallel: false,
                        unnumbered: seg.unnumbered,
                        city_hint: None,
                        rtt_ms: None,
                        members: seg.members.clone(),
                        aliases: Vec::new(),
                        targets: Vec::new(),
                    });
                    before.push(prev.map(|p| p.0));
                    arrive.push(Vec::new());
                    id
                }
            };
            let far = seg.members.iter().find_map(|m| m.best_ms);
            arrive[id].extend(far);
            if !groups[id].targets.iter().any(|t| t == target) {
                groups[id].targets.push(target.to_string());
            }
            if let Some((from, was)) = prev {
                let at = links.iter().position(|l| l.from == from && l.to == id).unwrap_or_else(|| {
                    links.push(Link {
                        from,
                        to: id,
                        count: 0,
                        targets: Vec::new(),
                        rtt_step_ms: None,
                        kind: LinkKind::Unknown,
                        rel: None,
                        handoff: false,
                        silent_hops: 0,
                    });
                    steps.push(Vec::new());
                    fed.push(None);
                    links.len() - 1
                });
                links[at].count += 1;
                links[at].targets.push(target.to_string());
                if groups[from].role == Role::Exchange {
                    fed[at] = fed[at].or(last_named);
                }
                if silence > 0 {
                    links[at].silent_hops = links[at].silent_hops.max(silence);
                    silence = 0;
                }
                let near = was.members.iter().find_map(|m| m.best_ms);
                if let (Some(near), Some(far)) = (near, far) {
                    steps[at].push(far - near);
                }
            }
            mine.push(id);
            if groups[id].role != Role::Exchange {
                last_named = groups[id].asn.or(last_named);
            }
            prev = Some((id, seg));
        }
        ids.push(mine);
    }
    for (g, at) in groups.iter_mut().zip(arrive) {
        g.rtt_ms = median(at.into_iter()).map(|v| (v * 10.0).round() / 10.0);
        g.label = label(g.role, g.asn, g.holder.as_deref(), g.ix_name.as_deref());
        g.city_hint = g.members.iter().find_map(|m| m.hint.clone());
        g.aliases = aliases(&g.members);
    }
    for ((l, step), fed) in links.iter_mut().zip(steps).zip(fed) {
        let to = &groups[l.to];
        // Below zero is noise between two best-of-four readings, not a shortcut.
        l.rtt_step_ms = median(step.into_iter()).map(|v| (v * 10.0).round() / 10.0).map(|v| if v > 0.0 { v } else { 0.0 });
        l.kind = match to.role {
            Role::Exchange => LinkKind::Exchange { ix_id: to.ix_id.unwrap_or_default() },
            // Nobody answered from the destination: where the traffic was handed to it is not known.
            Role::Content if to.members.is_empty() => LinkKind::Unknown,
            Role::Content => LinkKind::Direct,
            Role::Carrier => LinkKind::ViaTransit,
            _ => LinkKind::Unknown,
        };
        // The exchange node is not a network, so its links keep the role rule. Elsewhere a listed pair decides.
        let from_asn = if groups[l.from].role == Role::Exchange { fed } else { groups[l.from].asn };
        // A destination nobody answered from has no known hand-off either.
        if to.role != Role::Exchange && !(to.role == Role::Content && to.members.is_empty()) {
            l.rel = from_asn.zip(to.asn).and_then(|(a, b)| rels.get(a, b));
        }
        l.kind = match l.rel {
            Some(Rel::Transit) => LinkKind::ViaTransit,
            Some(Rel::Peer | Rel::Customer) => LinkKind::Direct,
            None => l.kind,
        };
        l.handoff = groups[l.from].role == Role::Isp && to.role != Role::Isp;
    }
    (groups, links, ids)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::trace::Hop;

    pub fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    pub fn hop(ttl: u8, addr: Option<&str>) -> Hop {
        let addrs: Vec<IpAddr> = addr.map(ip).into_iter().collect();
        Hop {
            ttl,
            private: addrs.first().is_some_and(is_private),
            recv: addrs.len() as u32 * 4,
            addrs,
            asn: None,
            holder: None,
            samples_ms: vec![],
            best_ms: addr.map(|_| f64::from(ttl) * 2.0),
            avg_ms: None,
            last_ms: None,
            sent: 4,
        }
    }

    pub fn trace(target: &str, hops: &[Option<&str>]) -> Trace {
        let hops = hops.iter().enumerate().map(|(i, a)| hop(i as u8 + 1, *a)).collect();
        Trace { target: ip(target), protocol: "icmp", rounds: 4, reached: true, elapsed_ms: 0.0, hops }
    }

    /// 198.51.100.0/24 is the ISP (AS100), 203.0.113.0/24 a carrier (AS200), 192.0.2.0/24 the content network (AS300).
    pub fn pfx() -> Pfx2as {
        let net = |s: &str| u32::from(s.parse::<Ipv4Addr>().unwrap());
        Pfx2as::from_rows(
            [(net("198.51.100.0"), 24, 100, false), (net("203.0.113.0"), 24, 200, false), (net("192.0.2.0"), 24, 300, false)].into_iter(),
            std::iter::empty(),
        )
    }

    pub fn cx<'a>(pfx: &'a Pfx2as, pdb: &'a Pdb, names: &'a HashMap<IpAddr, String>) -> Ctx<'a> {
        static NO_REGISTRY: std::sync::LazyLock<HashMap<IpAddr, Block>> = std::sync::LazyLock::new(HashMap::new);
        static NO_HOLDERS: std::sync::LazyLock<HashMap<u32, String>> = std::sync::LazyLock::new(HashMap::new);
        Ctx { pfx, pdb, home: Some(100), names, you: None, registry: &NO_REGISTRY, holders: &NO_HOLDERS }
    }

    #[test]
    fn consecutive_hops_in_one_network_group_and_a_return_shows_twice() {
        let mut pdb = Pdb::default();
        pdb.add_ix(31, "Equinix Ashburn");
        pdb.add_lan(31, "206.126.236.0/22");
        let (pfx, names) = (pfx(), HashMap::new());
        // ISP, ISP, IX, CDN, CDN is three groups.
        let t = trace(
            "192.0.2.9",
            &[
                Some("192.168.1.1"),
                Some("198.51.100.1"),
                Some("198.51.100.2"),
                Some("206.126.237.148"),
                Some("192.0.2.1"),
                Some("192.0.2.9"),
            ],
        );
        let segs = group(&t, &cx(&pfx, &pdb, &names));
        let roles: Vec<Role> = segs.iter().map(|s| s.role).collect();
        assert_eq!(roles, [Role::Home, Role::Isp, Role::Exchange, Role::Content]);
        assert_eq!(segs[1].members.len(), 2);
        assert_eq!(segs[2].ix, Some((31, "Equinix Ashburn".to_string())));
        // ISP, X, ISP, CDN keeps both ISP groups.
        let t = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("198.51.100.2"), Some("192.0.2.9")]);
        let segs = group(&t, &cx(&pfx, &pdb, &names));
        assert_eq!(segs.iter().map(|s| s.role).collect::<Vec<_>>(), [Role::Isp, Role::Carrier, Role::Isp, Role::Content]);
    }

    #[test]
    fn private_hops_are_home_at_the_start_and_unnumbered_only_between_agreeing_neighbours() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let t = trace(
            "192.0.2.9",
            &[
                Some("10.0.0.1"),
                Some("198.51.100.1"),
                Some("10.9.9.9"),
                Some("198.51.100.2"),
                Some("203.0.113.1"),
                Some("10.8.8.8"),
                Some("192.0.2.9"),
            ],
        );
        let segs = group(&t, &cx(&pfx, &pdb, &names));
        assert_eq!(segs.iter().map(|s| s.role).collect::<Vec<_>>(), [Role::Home, Role::Isp, Role::Carrier, Role::Unknown, Role::Content]);
        assert!(segs[1].unnumbered && segs[1].members.len() == 3 && !segs[3].unnumbered);
    }

    #[test]
    fn leading_silence_is_dropped_and_silence_inside_a_network_stays_in_it() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let t = trace("192.0.2.9", &[None, Some("10.0.0.1"), Some("198.51.100.1"), None, Some("198.51.100.2"), None, Some("192.0.2.9")]);
        let segs = group(&t, &cx(&pfx, &pdb, &names));
        assert_eq!(segs.iter().map(|s| s.role).collect::<Vec<_>>(), [Role::Home, Role::Isp, Role::Silent, Role::Content]);
        assert_eq!((segs[0].ttl_from, segs[1].ttl_from, segs[1].ttl_to, segs[1].members.len()), (2, 3, 5, 2));
    }

    pub fn block(start: &str, end: &str, org: &str) -> Block {
        Block {
            start: ip(start),
            end: ip(end),
            handle: format!("{start} - {end}"),
            name: "EX-BLOCK".into(),
            org: Some(org.into()),
            rir: "RIPE".into(),
            updated: "2022-10-29T00:51:39Z".into(),
            fetched_at: 1_777_000_000_000,
        }
    }

    #[test]
    fn an_unknown_network_is_named_for_its_registered_holder_and_holders_stay_apart() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let mut reg = HashMap::new();
        reg.insert(ip("203.0.114.5"), block("203.0.114.0", "203.0.114.255", "Example Networks Limited"));
        reg.insert(ip("203.0.115.5"), block("203.0.115.0", "203.0.115.255", "Other Networks SA"));
        let cx = Ctx { registry: &reg, ..cx(&pfx, &pdb, &names) };
        let a = group(&trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.114.5"), Some("192.0.2.9")]), &cx);
        let b = group(&trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.115.5"), Some("192.0.2.9")]), &cx);
        let c = group(&trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.116.5"), Some("192.0.2.9")]), &cx);
        assert_eq!((a[1].role, a[1].holder.as_deref()), (Role::Unknown, Some("Example Networks Limited")));
        assert_eq!(a[1].registry.as_ref().map(|b| b.name.as_str()), Some("EX-BLOCK"));
        assert_eq!((c[1].role, c[1].holder.as_deref(), c[1].registry.is_none()), (Role::Unknown, None, true));
        let (groups, _, _) = merge(&[("a", a), ("b", b), ("c", c)], &AsRel::empty());
        let labels: Vec<&str> = groups.iter().filter(|g| g.role == Role::Unknown).map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["Example Networks Limited (registry)", "Other Networks SA (registry)", "Unknown network"]);
    }

    #[test]
    fn routers_that_differ_at_one_hop_are_one_parallel_group() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let a = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")]);
        let b = trace("192.0.2.10", &[Some("198.51.100.1"), Some("203.0.113.2"), Some("192.0.2.10")]);
        let paths = [("a", group(&a, &cx(&pfx, &pdb, &names))), ("b", group(&b, &cx(&pfx, &pdb, &names)))];
        let (groups, links, ids) = merge(&paths, &AsRel::empty());
        // ISP, carrier (two routers), content.
        assert_eq!(groups.len(), 3);
        assert_eq!(ids, [vec![0, 1, 2], vec![0, 1, 2]]);
        assert!(groups[1].parallel && groups[1].members.len() == 2 && groups[1].targets == ["a", "b"]);
        assert_eq!(links.len(), 2);
        assert_eq!((links[0].count, links[0].handoff, links[0].kind), (2, true, LinkKind::ViaTransit));
        assert_eq!((links[1].count, links[1].handoff, links[1].kind), (2, false, LinkKind::Direct));
        // Round trip at the far side's first router minus the near side's: 4 ms (hop 2) minus 2 ms (hop 1).
        assert_eq!(links[0].rtt_step_ms, Some(2.0));
        assert_eq!(groups.iter().map(|g| g.rtt_ms).collect::<Vec<_>>(), [Some(2.0), Some(4.0), Some(6.0)]);
    }

    #[test]
    fn a_network_reached_two_ways_is_one_box_with_a_link_from_each_side() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        // One trace reaches the content network through the carrier, the other straight from the ISP.
        let a = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")]);
        let b = trace("192.0.2.10", &[Some("198.51.100.1"), Some("192.0.2.10")]);
        let paths = [("a", group(&a, &cx(&pfx, &pdb, &names))), ("b", group(&b, &cx(&pfx, &pdb, &names)))];
        let (groups, links, ids) = merge(&paths, &AsRel::empty());
        assert_eq!(groups.iter().map(|g| g.role).collect::<Vec<_>>(), [Role::Isp, Role::Carrier, Role::Content]);
        assert_eq!(ids, [vec![0, 1, 2], vec![0, 2]]);
        assert_eq!(groups[2].targets, ["a", "b"]);
        assert!(!groups[2].parallel, "routers on different sides are not parallel");
        let into: Vec<(usize, usize, u32)> = links.iter().filter(|l| l.to == 2).map(|l| (l.from, l.to, l.count)).collect();
        assert_eq!(into, [(1, 2, 1), (0, 2, 1)]);
        // Carried through for a third target, the content network is still that one box, and still content.
        let c = trace("203.0.114.9", &[Some("198.51.100.1"), Some("192.0.2.1"), Some("203.0.114.9")]);
        let c = group(&c, &cx(&pfx, &pdb, &names));
        assert_eq!(c[1].role, Role::Carrier);
        let (groups, _, ids) = merge(&[("c", c), ("a", paths[0].1.clone())], &AsRel::empty());
        assert_eq!(groups.iter().map(|g| g.role).collect::<Vec<_>>(), [Role::Isp, Role::Content, Role::Unknown, Role::Carrier]);
        assert_eq!(ids, [vec![0, 1, 2], vec![0, 3, 1]]);
    }

    #[test]
    fn a_second_visit_or_another_city_keeps_a_network_apart() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        // ISP, carrier, ISP, content: the ISP's second visit is its own box.
        let t = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("198.51.100.2"), Some("192.0.2.9")]);
        let (groups, _, ids) = merge(&[("a", group(&t, &cx(&pfx, &pdb, &names)))], &AsRel::empty());
        assert_eq!(groups.iter().map(|g| g.role).collect::<Vec<_>>(), [Role::Isp, Role::Carrier, Role::Isp, Role::Content]);
        assert_eq!(ids, [vec![0, 1, 2, 3]]);
        // The same carrier decoded to two cities stays two boxes; with no hint it joins the first.
        let a = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")]);
        let b = trace("192.0.2.10", &[Some("198.51.100.1"), Some("203.0.113.2"), Some("192.0.2.10")]);
        let c = trace("192.0.2.11", &[Some("198.51.100.1"), Some("203.0.113.3"), Some("192.0.2.11")]);
        let cx = cx(&pfx, &pdb, &names);
        let (mut a, mut b, c) = (group(&a, &cx), group(&b, &cx), group(&c, &cx));
        a[1].members[0].hint = Some("Ashburn".into());
        b[1].members[0].hint = Some("Chicago".into());
        let (groups, _, _) = merge(&[("a", a), ("b", b), ("c", c)], &AsRel::empty());
        let carriers: Vec<(Option<&str>, usize)> =
            groups.iter().filter(|g| g.role == Role::Carrier).map(|g| (g.city_hint.as_deref(), g.members.len())).collect();
        assert_eq!(carriers, [(Some("Ashburn"), 2), (Some("Chicago"), 1)]);
    }

    #[test]
    fn silence_between_two_networks_goes_on_the_link_and_silence_at_the_end_keeps_its_box() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let cx = cx(&pfx, &pdb, &names);
        // ISP, two silent hops, content: one link, its kind from the far side, the hidden hops on it.
        let a = trace("192.0.2.9", &[Some("198.51.100.1"), None, None, Some("192.0.2.9")]);
        let (groups, links, ids) = merge(&[("a", group(&a, &cx))], &AsRel::empty());
        assert_eq!(groups.iter().map(|g| g.role).collect::<Vec<_>>(), [Role::Isp, Role::Content]);
        assert_eq!(ids, [vec![0, 0, 1]]);
        assert_eq!((links[0].from, links[0].to, links[0].silent_hops, links[0].kind, links[0].handoff), (0, 1, 2, LinkKind::Direct, true));
        // Another trace that sees the two meet joins the same link; the hidden hops stay on it.
        let b = trace("192.0.2.10", &[Some("198.51.100.1"), Some("192.0.2.10")]);
        let (_, links, _) = merge(&[("a", group(&a, &cx)), ("b", group(&b, &cx))], &AsRel::empty());
        assert_eq!((links[0].count, links[0].silent_hops, links[0].kind), (2, 2, LinkKind::Direct));
        // A trace that ends in silence keeps the box: it is the only sign of where it stopped.
        let c = trace("192.0.2.9", &[Some("198.51.100.1"), None, None]);
        let (groups, links, _) = merge(&[("c", group(&c, &cx))], &AsRel::empty());
        assert_eq!(groups.iter().map(|g| g.role).collect::<Vec<_>>(), [Role::Isp, Role::Silent]);
        assert_eq!(links[0].silent_hops, 0);
    }

    #[test]
    fn a_destination_that_never_answers_is_drawn_after_the_last_network_that_did() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let holders = HashMap::from([(300, "Example CDN".to_string())]);
        let cx = Ctx { holders: &holders, ..cx(&pfx, &pdb, &names) };
        // ISP, carrier, then nothing: the trace was cut at the carrier's last answer.
        let mut t = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("203.0.113.2")]);
        t.reached = false;
        let segs = group(&t, &cx);
        assert_eq!(
            segs.iter().map(|s| (s.role, s.asn, s.members.len())).collect::<Vec<_>>(),
            [(Role::Isp, Some(100), 1), (Role::Carrier, Some(200), 2), (Role::Content, Some(300), 0)]
        );
        assert_eq!((segs[2].ttl_from, segs[2].holder.as_deref()), (4, Some("Example CDN")));
        let (groups, links, ids) = merge(&[("a", segs)], &AsRel::empty());
        assert_eq!(ids, [vec![0, 1, 2]]);
        assert_eq!(
            (groups[2].label.as_str(), groups[2].targets.as_slice(), groups[2].rtt_ms),
            ("Example CDN", &["a".to_string()][..], None)
        );
        assert_eq!((links[1].kind, links[1].handoff, links[1].rtt_step_ms), (LinkKind::Unknown, false, None));
        // A trace that does reach it joins the same box, and the link into it is known again.
        let b = trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")]);
        let (groups, links, _) = merge(&[("a", group(&t, &cx)), ("b", group(&b, &cx))], &AsRel::empty());
        assert_eq!(groups.len(), 3);
        assert_eq!((groups[2].members.len(), links[1].kind), (1, LinkKind::Direct));
        // Reached, nothing is added.
        assert_eq!(group(&b, &cx).len(), 3);
    }

    #[test]
    fn a_listed_relationship_sets_the_link_kind_and_an_unlisted_pair_keeps_the_role_rule() {
        let (pfx, names) = (pfx(), HashMap::new());
        let mut pdb = Pdb::default();
        pdb.add_ix(31, "Equinix Ashburn");
        pdb.add_lan(31, "206.126.236.0/22");
        let cx = cx(&pfx, &pdb, &names);
        let kinds = |hops: &[Option<&str>], rels: &AsRel| {
            let (_, links, _) = merge(&[("a", group(&trace("192.0.2.9", hops), &cx))], rels);
            links.iter().map(|l| (l.kind, l.rel)).collect::<Vec<_>>()
        };
        // A carrier the ISP peers with is a direct link, not transit.
        let carrier = [Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")];
        let peer = AsRel::from_rows([(100, 200, 0)]);
        assert_eq!(kinds(&carrier, &peer)[0], (LinkKind::Direct, Some(Rel::Peer)));
        // A content network the ISP buys transit from is via transit.
        let content = [Some("198.51.100.1"), Some("192.0.2.9")];
        let transit = AsRel::from_rows([(300, 100, -1)]);
        assert_eq!(kinds(&content, &transit)[0], (LinkKind::ViaTransit, Some(Rel::Transit)));
        // An unlisted pair keeps the role rule and carries no relationship.
        assert_eq!(kinds(&carrier, &AsRel::empty())[0], (LinkKind::ViaTransit, None));
        assert_eq!(kinds(&content, &AsRel::empty())[0], (LinkKind::Direct, None));
        // Out of an exchange node the near side is the network before it; into the node nothing is asked.
        let ix = [Some("198.51.100.1"), Some("206.126.237.148"), Some("192.0.2.9")];
        let customer = AsRel::from_rows([(100, 300, -1)]);
        let out = kinds(&ix, &customer);
        assert_eq!(out[0].1, None);
        assert!(matches!(out[0].0, LinkKind::Exchange { .. }));
        assert_eq!(out[1], (LinkKind::Direct, Some(Rel::Customer)));
        assert_eq!(kinds(&ix, &AsRel::empty())[1], (LinkKind::Direct, None));
    }

    #[test]
    fn the_longest_prefix_wins_and_private_space_never_matches() {
        let net = |s: &str| u32::from(s.parse::<Ipv4Addr>().unwrap());
        let t = Pfx2as::from_rows(
            [
                (net("8.0.0.0"), 8, 1, false),
                (net("8.8.0.0"), 16, 2, false),
                (net("8.8.8.0"), 24, 3, true),
                (net("10.0.0.0"), 8, 4, false),
                (net("100.64.0.0"), 10, 5, false),
                (net("192.168.0.0"), 16, 6, false),
                (net("169.254.0.0"), 16, 7, false),
            ]
            .into_iter(),
            [(0x2001_0db8_u128 << 96, 32, 64496, false), (0x2001_0db8_0001_u128 << 80, 48, 64497, true)].into_iter(),
        );
        assert_eq!(t.lookup(ip("2001:db8:1::5")), Some(Match { asn: 64497, moas: true, prefix: "2001:db8:1::/48".into() }));
        assert_eq!(t.lookup(ip("2001:db8:2::5")), Some(Match { asn: 64496, moas: false, prefix: "2001:db8::/32".into() }));
        assert_eq!(t.lookup(ip("2001:db9::1")), None);
        assert_eq!(t.lookup(ip("fd00::1")), None);
        assert_eq!(t.lookup(ip("8.8.8.8")), Some(Match { asn: 3, moas: true, prefix: "8.8.8.0/24".into() }));
        assert_eq!(t.asn_of(ip("8.8.4.4")), Some(2));
        assert_eq!(t.asn_of(ip("8.1.1.1")), Some(1));
        assert_eq!(t.asn_of(ip("9.9.9.9")), None);
        for private in ["10.1.2.3", "100.64.1.1", "192.168.1.1", "169.254.1.1", "127.0.0.1", "::1"] {
            assert_eq!(t.lookup(ip(private)), None, "{private}");
        }
    }

    #[test]
    fn table_lines_keep_the_first_origin_and_flag_several() {
        assert_eq!(parse_line("1.0.0.0\t24\t13335"), Some(Row::V4(u32::from(Ipv4Addr::new(1, 0, 0, 0)), 24, 13335, false)));
        assert_eq!(parse_line("1.1.8.0\t24\t149511_138421"), Some(Row::V4(u32::from(Ipv4Addr::new(1, 1, 8, 0)), 24, 149511, true)));
        assert_eq!(parse_line("15.0.64.0\t18\t8035,13979"), Some(Row::V4(u32::from(Ipv4Addr::new(15, 0, 64, 0)), 18, 8035, true)));
        assert_eq!(parse_line("15.0.64.0\t40\t1"), None);
        assert_eq!(parse_line("2001::\t32\t6939"), Some(Row::V6(0x2001_u128 << 112, 32, 6939, false)));
        assert_eq!(parse_line("2001:db8::\t48\t1_2"), Some(Row::V6(0x2001_0db8_u128 << 96, 48, 1, true)));
        assert_eq!(parse_line("2001::\t129\t1"), None);
        assert_eq!(newest("# Format: 1\n1\t100\t2026/10/a.gz\n2\t200\t2026/10/b.gz\n"), Some("2026/10/b.gz"));
    }

    /// Attribution takes the prefix table and PeeringDB already loaded and no client, so it cannot send a hop address
    /// anywhere: there is nothing in its signature to send it with.
    #[test]
    fn attribution_needs_only_loaded_tables() {
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let mut t = trace("192.0.2.9", &[Some("10.0.0.1"), Some("198.51.100.1"), None, Some("192.0.2.9")]);
        attribute(&mut t, &pfx);
        let asns: Vec<Option<u32>> = t.hops.iter().map(|h| h.asn).collect();
        assert_eq!(asns, [None, Some(100), None, Some(300)]);
        let segs = group(&t, &cx(&pfx, &pdb, &names));
        assert_eq!(segs.len(), 4);
    }

    #[test]
    fn routes_compare_by_the_networks_and_exchanges_they_cross() {
        let mut pdb = Pdb::default();
        pdb.add_ix(31, "Equinix Ashburn");
        pdb.add_lan(31, "206.126.236.0/22");
        let net = |s: &str| u32::from(s.parse::<Ipv4Addr>().unwrap());
        let pfx = Pfx2as::from_rows(
            [
                (net("198.51.100.0"), 24, 100, false),
                (net("203.0.113.0"), 24, 200, false),
                (net("192.0.2.0"), 24, 300, false),
                (net("93.184.216.0"), 24, 400, false),
            ]
            .into_iter(),
            std::iter::empty(),
        );
        let names = HashMap::new();
        let key = |hops: &[Option<&str>]| {
            let mut t = trace("192.0.2.9", hops);
            attribute(&mut t, &pfx);
            route_key(&group(&t, &cx(&pfx, &pdb, &names)))
        };
        let plain = key(&[Some("192.168.1.1"), Some("198.51.100.1"), Some("203.0.113.1"), Some("192.0.2.9")]);
        // A silent hop, and a second router in the same network, change nothing.
        let quiet = key(&[Some("192.168.1.1"), Some("198.51.100.1"), None, Some("203.0.113.1"), Some("203.0.113.2"), Some("192.0.2.9")]);
        assert_eq!(plain, quiet);
        assert_eq!(plain.len(), 3);
        // An extra transit network, or an exchange instead, is another route.
        let extra = key(&[Some("198.51.100.1"), Some("203.0.113.1"), Some("93.184.216.1"), Some("192.0.2.9")]);
        assert_ne!(plain, extra);
        let ix = key(&[Some("192.168.1.1"), Some("198.51.100.1"), Some("206.126.237.148"), Some("192.0.2.9")]);
        assert_ne!(plain, ix);
        assert!(ix.iter().any(|k| k.2 == Some(31)));
    }

    #[test]
    fn an_owner_is_only_named_when_it_is_another_organisation() {
        assert!(distinct_owner(Some("CROWNCASTLE-AS46887 - Crown Castle Fiber LLC"), "Zayo Bandwidth"));
        assert!(!distinct_owner(Some("ZAYO-6461 - Zayo Bandwidth"), "Zayo Bandwidth"));
        assert!(!distinct_owner(Some("Zayo Bandwidth, US"), "Zayo"));
        assert!(!distinct_owner(Some("Cloudflare, Inc."), "CLOUDFLARE INC"));
        assert!(distinct_owner(None, "Example, Inc."));
        assert!(!distinct_owner(None, " "));
    }
}
