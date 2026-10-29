//! What the map claims about each hand-off, and how sure it is.
//!
//! A hand-off is the link where traffic leaves your ISP. What a trace shows is addresses; who owns the link between
//! two of them is a matter of record (PeeringDB's list of exchange ports), inference (two addresses of one /30
//! announced by different networks), or neither. Each claim says which, in plain words, and carries the evidence
//! behind it with its age. The wording is fixed so the strength shows in the sentence: only a PeeringDB entry earns
//! the word "peering", a prefix announced by several networks never earns High, and a city is "consistent with",
//! never "in".

use super::asrel::Rel;
use super::attr::{Group, Link, LinkKind, Role, Seg, same_link, short_holder};
use super::pdb::Pdb;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Weakest first so the weakest of several is the minimum.
    Low,
    Medium,
    High,
}

/// One piece of what a claim rests on.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum Evidence {
    /// Seen in the trace.
    Observed(String),
    /// Read from a list someone else keeps, with the date its entry was last edited.
    Listed { source: &'static str, updated: Option<String>, text: String },
    /// Worked out here from what was seen and listed.
    Inferred(String),
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Claim {
    /// The target whose trace this is about.
    pub target: String,
    /// The index in the run's links of the hand-off.
    pub link: usize,
    pub tier: Tier,
    pub text: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Headline {
    pub text: String,
    pub tier: Tier,
}

/// One trace as `say` reads it: the target, its runs, and the group each run ended up in.
pub struct Path<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub segs: &'a [Seg],
    pub groups: &'a [usize],
}

/// A year-month-day from days since 1970-01-01 (Howard Hinnant's civil-from-days).
pub(crate) fn date_of_ms(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
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

/// The date part of PeeringDB's timestamp (`2026-04-02T08:30:00Z`).
fn day(updated: &str) -> &str {
    updated.get(..10).unwrap_or(updated)
}

fn isp_name(s: &Seg) -> String {
    s.holder.as_deref().map(|h| short_holder(h).to_string()).or(s.asn.map(|a| format!("AS{a}"))).unwrap_or_else(|| "your ISP".into())
}

fn net_name(s: &Seg) -> String {
    s.holder
        .as_deref()
        .map(|h| short_holder(h).to_string())
        .or(s.asn.map(|a| format!("AS{a}")))
        .unwrap_or_else(|| "the next network".into())
}

/// Everything about one trace's hand-off the claim needs.
fn claim(p: &Path, i: usize, links: &[Link], pdb: &Pdb, rel_month: Option<&str>) -> Option<Claim> {
    let (isp, next) = (&p.segs[i], p.segs.get(i + 1)?);
    // Silence after the ISP sits on the link to the next group that answers.
    let to = (i + 1..p.segs.len()).find(|j| p.segs[*j].role != Role::Silent).map_or(p.groups[i + 1], |j| p.groups[j]);
    let link = links.iter().position(|l| l.from == p.groups[i] && l.to == to)?;
    let near = isp.members.last()?;
    let isp_n = isp_name(isp);
    let synced = pdb.synced_at.map(date_of_ms);
    let mut ev = vec![Evidence::Observed(format!("Hop {} answered from {}.", near.ttl, near.addr))];
    if let Some(prefix) = &near.prefix {
        let asn = isp.asn.map(|a| format!("AS{a}")).unwrap_or_default();
        ev.push(Evidence::Inferred(format!("{} is in {prefix}, announced by {asn} (CAIDA prefix-to-AS).", near.addr)));
    }
    let listed = |source: &'static str, updated: &str, text: String| {
        let synced = synced.as_deref().map(|d| format!(", synced {d}")).unwrap_or_default();
        Evidence::Listed { source, updated: Some(updated.to_string()), text: format!("{text} (entry edited {}{synced}).", day(updated)) }
    };
    // The network after an exchange, when there is one: it is who the exchange hop hands to.
    let beyond = match next.role {
        Role::Exchange => p.segs.get(i + 2).filter(|s| s.asn.is_some()),
        Role::Silent => p.segs.get(i + 2),
        _ => Some(next),
    };
    let mut far_asn = beyond.and_then(|s| s.asn);
    let (mut tier, mut text);
    let rel = links[link].rel;
    // What the relationship table says about two networks, as a citation.
    let cite = |rel: Rel, a: Option<u32>, b: Option<u32>| {
        let (a, b) = (a.map(|a| format!("AS{a}")).unwrap_or_default(), b.map(|b| format!("AS{b}")).unwrap_or_default());
        let what = match rel {
            Rel::Transit => format!("{a} buys transit from {b}"),
            Rel::Peer => format!("{a} and {b} peer"),
            Rel::Customer => format!("{b} is a customer of {a}"),
        };
        let month = rel_month.map(|m| format!(", {m}")).unwrap_or_default();
        Evidence::Listed {
            source: "CAIDA AS Relationships",
            updated: rel_month.map(str::to_string),
            text: format!("{what} (inferred from BGP routing{month})."),
        }
    };
    let first = next.members.first();
    let port = if next.role == Role::Exchange { first.and_then(|m| pdb.port_of(m.addr)) } else { None };
    let moas = near.moas || first.is_some_and(|m| m.moas);
    if next.role == Role::Exchange {
        let ix = next.ix.as_ref().map(|(_, n)| n.as_str()).filter(|n| !n.is_empty()).unwrap_or("an");
        if let (Some(port), Some(m)) = (port, first) {
            far_asn = far_asn.or(Some(port.asn)).filter(|a| Some(*a) != isp.asn).or(Some(port.asn));
            let owns = if Some(port.asn) == isp.asn {
                format!("{isp_n}'s")
            } else if beyond.and_then(|s| s.asn) == Some(port.asn) {
                format!("{}'s", beyond.map(net_name).unwrap_or_default())
            } else {
                format!("AS{}'s", port.asn)
            };
            ev.push(listed("PeeringDB netixlan", &port.updated, format!("{} is listed as AS{}'s port on {ix}", m.addr, port.asn)));
            let matches = Some(port.asn) == isp.asn || beyond.and_then(|s| s.asn) == Some(port.asn);
            if matches && !moas {
                tier = Tier::High;
                text = format!("Traffic to {} leaves {isp_n} at {owns} port on the {ix} exchange, per PeeringDB.", p.name);
            } else {
                tier = Tier::Medium;
                text = format!(
                    "This hop is {owns} port on the {ix} exchange, per PeeringDB; it is not clear that it is where traffic to {} leaves {isp_n}.",
                    p.name
                );
            }
        } else {
            tier = Tier::Medium;
            text = format!("This hop is on the {ix} peering LAN; PeeringDB doesn't say who owns it.");
            if let Some(lan) = first.and_then(|m| pdb.lan_of(m.addr)) {
                ev.push(listed(
                    "PeeringDB ixpfx",
                    &lan.updated,
                    format!("{} is in {}, an exchange LAN", first.map(|m| m.addr).unwrap(), lan.prefix),
                ));
            }
        }
    } else if let (true, Some(m), Some(a), Some(b)) = (
        matches!(next.role, Role::Carrier | Role::Content) && next.asn != isp.asn && next.ttl_from == near.ttl + 1,
        first,
        isp.asn,
        next.asn,
    ) {
        let direct = same_link(near.addr, m.addr) && !moas && near.prefix.is_some() && m.prefix.is_some();
        ev.push(Evidence::Inferred(format!(
            "{} is in {}, announced by AS{b}; {} is in {} (AS{a}).",
            m.addr,
            m.prefix.as_deref().unwrap_or("no announced prefix"),
            near.addr,
            near.prefix.as_deref().unwrap_or("no announced prefix"),
        )));
        if direct {
            tier = Tier::High;
            text = format!("Traffic to {} leaves {isp_n} on a direct link to {}.", p.name, net_name(next));
        } else {
            tier = Tier::Medium;
            text = format!("{isp_n} hands traffic to {} here; which side owns the link address is unclear.", net_name(next));
        }
    } else if matches!(next.role, Role::Carrier | Role::Content) && next.asn != isp.asn {
        tier = Tier::Medium;
        text = format!("{isp_n} hands traffic to {} here; which side owns the link address is unclear.", net_name(next));
    } else if let (Role::Unknown, Some(b), Some(m)) = (next.role, next.registry.as_ref(), first) {
        tier = Tier::Low;
        let who = b.holder();
        text = format!(
            "Traffic to {} leaves {isp_n} toward address space registered to {who}, per {}; no network announces it, so who runs these routers is not confirmed.",
            p.name, b.rir
        );
        ev.push(Evidence::Listed {
            source: b.source(),
            updated: Some(b.updated.clone()),
            text: format!(
                "{} is in {} ({} to {}), registered to {who} (entry edited {}, looked up {}).",
                m.addr,
                b.name,
                b.start,
                b.end,
                day(&b.updated),
                date_of_ms(b.fetched_at)
            ),
        });
    } else if next.role == Role::Silent
        && let Some(far) = beyond.filter(|s| s.role != Role::Silent)
    {
        // Routers that don't answer are hidden, not another network: the ISP's MPLS core, or the far side's border.
        // What is known is where the traffic comes out, how much further on, and whether both ends decode to one city.
        tier = Tier::Low;
        let n = next.ttl_to - next.ttl_from + 1;
        let out = far.members.first();
        let step = out.and_then(|m| m.best_ms).zip(near.best_ms).map(|(f, n)| (f - n).max(0.0));
        let city = near.hint.as_deref().zip(out.and_then(|m| m.hint.as_deref())).filter(|(a, b)| a == b).map(|(a, _)| a);
        let far_n = net_name(far);
        let rel = rel.filter(|_| far.asn.is_some());
        text = format!(
            "Traffic to {} leaves {isp_n} through {n} router{} that don't answer; the next to answer is in {far_n}{}{}.",
            p.name,
            if n == 1 { "" } else { "s" },
            match step {
                Some(s) if s < 1.0 => ", under a millisecond further on".to_string(),
                Some(s) => format!(", {s:.1} ms further on"),
                None => String::new(),
            },
            city.map(|c| format!(", both ends decoding to {c}")).unwrap_or_default()
        );
        text.push_str(match far.role {
            Role::Content => " Consistent with a direct link, private or a cache placed in the ISP's network; not confirmed.",
            Role::Carrier => " Consistent with a hand-off to that network, direct or through one that stays hidden; not confirmed.",
            _ => "",
        });
        if let Some(rel) = rel {
            text.push_str(&match rel {
                Rel::Peer => " The two peer directly per CAIDA, so the hidden routers are most likely in one of them.".to_string(),
                Rel::Transit => format!(" {far_n} is {isp_n}'s provider per CAIDA, so the hidden routers are most likely in one of them."),
                Rel::Customer => format!(" {far_n} is {isp_n}'s customer per CAIDA, so the hidden routers are most likely in one of them."),
            });
            tier = Tier::Medium;
            ev.push(cite(rel, isp.asn, far.asn));
        }
        if let Some(m) = out {
            ev.push(Evidence::Observed(format!(
                "Hops {} to {} did not answer; hop {} answered from {}.",
                next.ttl_from, next.ttl_to, m.ttl, m.addr
            )));
            if let (Some(prefix), Some(a)) = (&m.prefix, far.asn) {
                ev.push(Evidence::Inferred(format!("{} is in {prefix}, announced by AS{a} (CAIDA prefix-to-AS).", m.addr)));
            }
        }
    } else {
        tier = Tier::Low;
        text = format!("Traffic to {} leaves {isp_n} somewhere in the next two hops; the routers there don't answer.", p.name);
    }
    // A listed relationship says what the link is, in place of a guess from the far network's role.
    if let (Some(rel), true) = (rel, matches!(next.role, Role::Carrier | Role::Content) && next.asn != isp.asn) {
        let far_n = net_name(next);
        text = match rel {
            Rel::Transit => format!("Traffic to {} leaves {isp_n} on a transit link to {far_n}, its provider.", p.name),
            Rel::Peer => format!("Traffic to {} leaves {isp_n} on a peering link to {far_n}.", p.name),
            Rel::Customer => format!("Traffic to {} leaves {isp_n} on a direct link to {far_n}, a customer of {isp_n}.", p.name),
        };
        ev.push(cite(rel, isp.asn, next.asn));
    }
    // A building both networks list is a place a private link could be, no more.
    if tier != Tier::High
        && pdb.synced_at.is_some()
        && let (Some(a), Some(b)) = (isp.asn, far_asn.filter(|b| Some(*b) != isp.asn))
    {
        let far_n = beyond.map(net_name).unwrap_or_else(|| format!("AS{b}"));
        let shared = pdb.shared_facilities(a, b);
        match shared.as_slice() {
            [] => text.push_str(" PeeringDB lists no shared facility between these networks."),
            [(f, ua, ub)] => {
                text.push_str(&format!(
                    " {isp_n} and {far_n} both list {} ({}) in PeeringDB; if they interconnect privately it could be there. Not confirmed. Entries edited {} and {}.",
                    f.name,
                    f.city,
                    day(ua),
                    day(ub)
                ));
                ev.push(listed("PeeringDB netfac", ua.max(ub), format!("Both list {}", f.name)));
            }
            many => {
                let each: Vec<String> =
                    many.iter().map(|(f, ua, ub)| format!("{} ({}; entries edited {} and {})", f.name, f.city, day(ua), day(ub))).collect();
                text.push_str(&format!(
                    " {isp_n} and {far_n} both list {} buildings in PeeringDB: {}. If they interconnect privately it could be at one of them. Not confirmed.",
                    many.len(),
                    each.join("; ")
                ));
                let newest = many.iter().flat_map(|(_, a, b)| [a, b]).max().cloned().unwrap_or_default();
                ev.push(listed("PeeringDB netfac", &newest, format!("Both list {} buildings", many.len())));
            }
        }
    }
    // Unannounced space further along: name its registered holder, which says nothing about the hand-off itself.
    for b in p.segs.iter().skip(i + 2).filter(|s| s.role == Role::Unknown).filter_map(|s| s.registry.as_ref()) {
        let who = b.holder();
        text.push_str(&format!(" Further on, routers in address space registered to {who}, per {}; no network announces it.", b.rir));
        ev.push(Evidence::Listed {
            source: b.source(),
            updated: Some(b.updated.clone()),
            text: format!("{} is registered to {who} (entry edited {}, looked up {}).", b.name, day(&b.updated), date_of_ms(b.fetched_at)),
        });
    }
    Some(Claim { target: p.id.to_string(), link, tier, text, evidence: ev })
}

/// Whether the router at the hand-off and both its neighbours in time are silent: nothing then says where it is.
fn blind(segs: &[Seg], i: usize) -> bool {
    let ttl = segs[i].ttl_to;
    let silent = |s: Option<&Seg>, at: u8| s.is_some_and(|s| s.role == Role::Silent && s.ttl_from <= at && at <= s.ttl_to);
    // Silence before the first answer never becomes a segment, so a first segment that starts past TTL 1 follows it.
    let before = match i.checked_sub(1) {
        Some(j) => silent(segs.get(j), ttl.wrapping_sub(1)),
        None => segs[i].ttl_from > 1 && ttl == segs[i].ttl_from,
    };
    before && silent(segs.get(i + 1), ttl + 1)
}

/// The claims, one per trace that leaves the ISP, and the one-line headline over them. `home` is your ISP's number;
/// without it nothing can be said. The headline is withheld when every hand-off sits among routers that don't answer.
pub fn say(
    paths: &[Path],
    groups: &[Group],
    links: &[Link],
    home: Option<u32>,
    pdb: &Pdb,
    rel_month: Option<&str>,
) -> (Vec<Claim>, Option<Headline>) {
    let Some(home) = home else { return (Vec::new(), None) };
    let mut claims = Vec::new();
    let mut open = false;
    let mut isp = String::new();
    for p in paths {
        let Some(i) = p.segs.iter().rposition(|s| s.role == Role::Isp && s.asn == Some(home)) else { continue };
        if let Some(c) = claim(p, i, links, pdb, rel_month) {
            open |= !blind(p.segs, i);
            isp = isp_name(&p.segs[i]);
            claims.push(c);
        }
    }
    if claims.is_empty() || !open {
        return (claims, None);
    }
    let tier = claims.iter().map(|c| c.tier).min().unwrap_or(Tier::Low);
    let mut used: Vec<usize> = claims.iter().map(|c| c.link).collect();
    used.sort_unstable();
    used.dedup();
    let count = |f: fn(&Link) -> bool| used.iter().filter(|l| f(&links[**l])).count();
    let (at_ix, peering, transit, customers, direct) = (
        count(|l| matches!(l.kind, LinkKind::Exchange { .. })),
        count(|l| l.rel == Some(Rel::Peer)),
        count(|l| matches!(l.kind, LinkKind::ViaTransit)),
        count(|l| l.rel == Some(Rel::Customer)),
        count(|l| matches!(l.kind, LinkKind::Direct) && l.rel.is_none()),
    );
    let mut far: Vec<usize> = used.iter().map(|l| links[*l].to).collect();
    far.dedup();
    let n = far.len();
    let parts: Vec<String> = [
        (at_ix, "at exchanges"),
        (peering, "by peering"),
        (transit, "via transit"),
        (customers, "to their customers"),
        (direct, "on direct links"),
    ]
    .iter()
    .filter(|(k, _)| *k > 0)
    .map(|(k, w)| format!("{k} {w}"))
    .collect();
    let mut text = format!("{isp} hands your traffic to {n} network{}", if n == 1 { "" } else { "s" });
    if !parts.is_empty() {
        text.push_str(&format!(": {}", parts.join(", ")));
    }
    text.push('.');
    let near = used.iter().filter_map(|l| groups[links[*l].to].rtt_ms).reduce(f64::min);
    match near {
        Some(ms) if ms < 1.0 => text.push_str(" Nearest hand-off under a millisecond from you."),
        Some(ms) => text.push_str(&format!(" Nearest hand-off {ms} ms from you.")),
        None => {}
    }
    (claims, Some(Headline { text, tier }))
}

#[cfg(test)]
mod tests {
    use super::super::asrel::AsRel;
    use super::super::attr::tests::{cx, ip, pfx, trace};
    use super::super::attr::{group, merge};
    use super::*;
    use std::collections::HashMap;

    /// Say about one trace per target, the way a run does: group, merge, then claim.
    fn run(pdb: &Pdb, traces: &[(&str, &[Option<&str>])]) -> (Vec<Claim>, Option<Headline>) {
        run_with(pdb, traces, &AsRel::empty())
    }

    fn run_with(pdb: &Pdb, traces: &[(&str, &[Option<&str>])], rels: &AsRel) -> (Vec<Claim>, Option<Headline>) {
        let (pfx, names) = (pfx(), HashMap::new());
        let segs: Vec<(&str, Vec<Seg>)> = traces.iter().map(|(t, h)| (*t, group(&trace("192.0.2.9", h), &cx(&pfx, pdb, &names)))).collect();
        // Give the ISP and the carrier a holder, as the run does from the AS names.
        let segs: Vec<(&str, Vec<Seg>)> = segs
            .into_iter()
            .map(|(t, mut s)| {
                for seg in &mut s {
                    seg.holder = match seg.asn {
                        Some(100) => Some("EXAMPLENET - Example Net Inc".into()),
                        Some(200) => Some("Carrier Co".into()),
                        Some(300) => Some("Content Co".into()),
                        _ => None,
                    };
                }
                (t, s)
            })
            .collect();
        let (groups, links, ids) = merge(&segs, rels);
        let paths: Vec<Path> = segs.iter().zip(&ids).map(|((t, s), g)| Path { id: t, name: "Content Co", segs: s, groups: g }).collect();
        say(&paths, &groups, &links, Some(100), pdb, Some("October 2026"))
    }

    #[test]
    fn a_hand_off_into_registered_space_cites_the_registry_at_tier_low() {
        use super::super::attr::Ctx;
        use super::super::attr::tests::block;
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let mut reg = HashMap::new();
        reg.insert(ip("203.0.114.5"), block("203.0.114.0", "203.0.114.255", "Example Networks Limited"));
        let cx = Ctx { registry: &reg, ..cx(&pfx, &pdb, &names) };
        let segs = group(&trace("192.0.2.9", &[Some("198.51.100.1"), Some("203.0.114.5"), Some("192.0.2.9")]), &cx);
        let segs = [("a", segs)];
        let (groups, links, ids) = merge(&segs, &AsRel::empty());
        let paths = [Path { id: "a", name: "Content Co", segs: &segs[0].1, groups: &ids[0] }];
        let (claims, _) = say(&paths, &groups, &links, Some(100), &pdb, None);
        assert_eq!(claims.len(), 1);
        let c = &claims[0];
        assert_eq!(c.tier, Tier::Low);
        assert!(c.text.contains("registered to Example Networks Limited, per RIPE"), "{}", c.text);
        assert!(c.text.contains("not confirmed"), "{}", c.text);
        let cited = c.evidence.iter().any(|e| matches!(e, Evidence::Listed { source: "RIPE registry", updated: Some(u), text } if u.starts_with("2022-10-29") && text.contains("EX-BLOCK") && text.contains("entry edited 2022-10-29")));
        assert!(cited, "{:?}", c.evidence);
    }

    #[test]
    fn registered_space_further_along_is_named_without_changing_the_hand_off() {
        use super::super::attr::Ctx;
        use super::super::attr::tests::block;
        let (pfx, pdb, names) = (pfx(), Pdb::default(), HashMap::new());
        let mut reg = HashMap::new();
        reg.insert(ip("203.0.114.5"), block("203.0.114.0", "203.0.114.255", "Example Networks Limited"));
        let cx = Ctx { registry: &reg, ..cx(&pfx, &pdb, &names) };
        let segs = group(&trace("192.0.2.9", &[Some("198.51.100.1"), Some("192.0.2.1"), Some("203.0.114.5"), Some("192.0.2.9")]), &cx);
        let segs = [("a", segs)];
        let (groups, links, ids) = merge(&segs, &AsRel::empty());
        let paths = [Path { id: "a", name: "Content Co", segs: &segs[0].1, groups: &ids[0] }];
        let (claims, _) = say(&paths, &groups, &links, Some(100), &pdb, None);
        assert_eq!(claims.len(), 1);
        let c = &claims[0];
        assert_ne!(c.tier, Tier::Low, "{}", c.text);
        assert!(c.text.contains("Further on, routers in address space registered to Example Networks Limited, per RIPE"), "{}", c.text);
        let cited = c.evidence.iter().any(|e| matches!(e, Evidence::Listed { source: "RIPE registry", text, .. } if text.contains("EX-BLOCK is registered to Example Networks Limited")));
        assert!(cited, "{:?}", c.evidence);
    }

    fn words(text: &str, w: &str) -> bool {
        text.split(|c: char| !c.is_alphanumeric()).any(|x| x == w)
    }

    fn exchange() -> Pdb {
        let mut pdb = Pdb::default();
        pdb.add_ix(31, "Equinix Ashburn");
        pdb.add_lan(31, "206.126.236.0/22");
        pdb.add_port(31, 300, "206.126.237.148");
        pdb
    }

    #[test]
    fn a_listed_port_is_high_and_a_bare_lan_is_medium() {
        let pdb = exchange();
        let via = |ix: &'static str| [Some("198.51.100.1"), Some("198.51.100.2"), Some(ix), Some("192.0.2.9")];
        let (claims, head) = run(&pdb, &[("a", &via("206.126.237.148"))]);
        assert_eq!(claims[0].tier, Tier::High);
        assert_eq!(
            claims[0].text,
            "Traffic to Content Co leaves Example Net Inc at Content Co's port on the Equinix Ashburn exchange, per PeeringDB."
        );
        assert!(claims[0].evidence.iter().any(
            |e| matches!(e, Evidence::Listed { updated: Some(u), text, .. } if u.starts_with("2026-04-02") && text.contains("synced 2026-"))
        ));
        assert_eq!(head.map(|h| h.tier), Some(Tier::High));

        let (claims, _) = run(&pdb, &[("a", &via("206.126.236.7"))]);
        assert_eq!(claims[0].tier, Tier::Medium);
        assert!(claims[0].text.starts_with("This hop is on the Equinix Ashburn peering LAN; PeeringDB doesn't say who owns it."));
    }

    #[test]
    fn a_shared_link_is_high_and_others_are_medium_or_low_with_the_building_sentence() {
        let mut pdb = Pdb::default();
        pdb.add_listing(1, "Equinix DC2", 100, "2026-01-10T00:00:00Z");
        pdb.add_listing(1, "Equinix DC2", 200, "2026-01-20T00:00:00Z");
        // 203.0.113.1 and .2 are one /30 across ISP and carrier.
        let (claims, _) = run(&pdb, &[("a", &[Some("198.51.100.1"), Some("203.0.113.2"), Some("203.0.113.5"), Some("192.0.2.9")])]);
        assert_eq!(claims[0].tier, Tier::Medium);
        assert!(claims[0].text.contains("hands traffic to Carrier Co here; which side owns the link address is unclear."));
        assert!(claims[0].text.contains(
            "Example Net Inc and Carrier Co both list Equinix DC2 (Ashburn) in PeeringDB; if they interconnect privately it could be there. Not confirmed. Entries edited 2026-01-10 and 2026-01-20."
        ));
        // The same pair on one /30 is a direct link.
        let (claims, _) =
            run(&Pdb::default(), &[("a", &[Some("198.51.100.1"), Some("198.51.100.2"), Some("203.0.113.1"), Some("192.0.2.9")])]);
        // 198.51.100.2 and 203.0.113.1 are not one /30, so this is still medium.
        assert_eq!(claims[0].tier, Tier::Medium);
        assert!(!claims[0].text.contains("PeeringDB"));
        let (claims, _) = run(&pdb, &[("a", &[Some("198.51.100.1"), None, None, Some("192.0.2.9")])]);
        assert_eq!(claims[0].tier, Tier::Low);
        assert!(claims[0].text.contains("through 2 routers that don't answer; the next to answer is in Content Co"));
        assert!(claims[0].text.contains("Consistent with a direct link"));
    }

    #[test]
    fn word_rules_moas_and_the_withheld_headline() {
        // Two networks announce 198.51.100.0/24 here, so the shared /30 is not enough.
        let net = |s: &str| u32::from(s.parse::<std::net::Ipv4Addr>().unwrap());
        let pfx2 = super::super::attr::Pfx2as::from_rows(
            [(net("198.51.100.0"), 24, 100, true), (net("203.0.113.0"), 24, 200, false), (net("192.0.2.0"), 24, 300, false)].into_iter(),
            std::iter::empty(),
        );
        let (names, pdb) = (HashMap::new(), Pdb::default());
        let t = trace("192.0.2.9", &[Some("198.51.100.1"), Some("198.51.100.2"), Some("203.0.113.1"), Some("192.0.2.9")]);
        let segs = group(&t, &cx(&pfx2, &pdb, &names));
        assert!(segs[0].members[0].moas);
        let (groups, links, ids) = merge(&[("a", segs.clone())], &AsRel::empty());
        let paths = [Path { id: "a", name: "Content Co", segs: &segs, groups: &ids[0] }];
        let (claims, _) = say(&paths, &groups, &links, Some(100), &pdb, None);
        assert!(claims.iter().all(|c| c.tier != Tier::High));

        // "peering" appears only on a claim that cites PeeringDB, and a city is never "in".
        let pdb = exchange();
        for hops in [
            [Some("198.51.100.1"), Some("198.51.100.2"), Some("206.126.236.7"), Some("192.0.2.9")],
            [Some("198.51.100.1"), Some("198.51.100.2"), Some("206.126.237.148"), Some("192.0.2.9")],
            [Some("198.51.100.1"), Some("198.51.100.2"), Some("203.0.113.1"), Some("192.0.2.9")],
            [Some("198.51.100.1"), None, None, Some("192.0.2.9")],
        ] {
            for c in run(&pdb, &[("a", &hops)]).0 {
                let cited = c.evidence.iter().any(|e| matches!(e, Evidence::Listed { source, .. } if source.starts_with("PeeringDB")));
                assert!(!words(&c.text, "peering") || cited, "{}", c.text);
                assert!(c.evidence.iter().all(|e| !matches!(e, Evidence::Listed { updated: None, .. })));
            }
        }

        // A hand-off among routers that never answer says nothing in the headline.
        let (claims, head) = run(&Pdb::default(), &[("a", &[None, Some("198.51.100.1"), None, Some("192.0.2.9")])]);
        assert_eq!(claims.len(), 1);
        assert!(head.is_none());
        let _ = ip("192.0.2.9");
    }

    #[test]
    fn the_headline_counts_networks_by_how_they_are_reached() {
        let pdb = exchange();
        let (claims, head) = run(
            &pdb,
            &[
                ("a", &[Some("198.51.100.1"), Some("198.51.100.2"), Some("206.126.237.148"), Some("192.0.2.9")]),
                ("b", &[Some("198.51.100.1"), Some("198.51.100.2"), Some("203.0.113.1"), Some("192.0.2.9")]),
            ],
        );
        assert_eq!(claims.len(), 2);
        let head = head.unwrap();
        assert_eq!(head.tier, Tier::Medium);
        assert!(head.text.starts_with("Example Net Inc hands your traffic to 2 networks: 1 at exchanges, 1 via transit."), "{}", head.text);
        assert_eq!(date_of_ms(1_790_000_000_000), "2026-09-21");
    }

    #[test]
    fn a_listed_relationship_changes_the_sentence_the_evidence_and_the_headline() {
        let hops = |a: Option<&'static str>| [Some("198.51.100.1"), Some("198.51.100.2"), a, Some("192.0.2.9")];
        let one = |rels: &AsRel, via: &'static str| run_with(&Pdb::default(), &[("a", &hops(Some(via)))], rels);
        let cited = |c: &Claim| {
            c.evidence
                .iter()
                .any(|e| matches!(e, Evidence::Listed { source: "CAIDA AS Relationships", updated: Some(m), .. } if m == "October 2026"))
        };
        let (claims, head) = one(&AsRel::from_rows([(100, 200, -1)]), "203.0.113.1");
        assert_eq!(
            claims[0].text,
            "Traffic to Content Co leaves Example Net Inc on a direct link to Carrier Co, a customer of Example Net Inc."
        );
        assert!(cited(&claims[0]), "{:?}", claims[0].evidence);
        assert!(head.unwrap().text.starts_with("Example Net Inc hands your traffic to 1 network: 1 to their customers."));
        let (claims, head) = one(&AsRel::from_rows([(200, 100, -1)]), "203.0.113.1");
        assert_eq!(claims[0].text, "Traffic to Content Co leaves Example Net Inc on a transit link to Carrier Co, its provider.");
        assert!(cited(&claims[0]));
        assert!(head.unwrap().text.contains("1 via transit"));
        let (claims, head) = one(&AsRel::from_rows([(100, 200, 0)]), "203.0.113.1");
        assert_eq!(claims[0].text, "Traffic to Content Co leaves Example Net Inc on a peering link to Carrier Co.");
        assert!(head.unwrap().text.contains("1 by peering"));
        // No relationship: still an inference.
        let (claims, _) = one(&AsRel::empty(), "203.0.113.1");
        assert!(!cited(&claims[0]));
    }

    #[test]
    fn a_silent_stretch_between_related_networks_reads_medium() {
        let silent = [Some("198.51.100.1"), None, None, Some("203.0.113.1"), Some("192.0.2.9")];
        let (claims, _) = run_with(&Pdb::default(), &[("a", &silent)], &AsRel::empty());
        assert_eq!(claims[0].tier, Tier::Low);
        let (claims, _) = run_with(&Pdb::default(), &[("a", &silent)], &AsRel::from_rows([(100, 200, -1)]));
        assert_eq!(claims[0].tier, Tier::Medium);
        assert!(
            claims[0]
                .text
                .contains("Carrier Co is Example Net Inc's customer per CAIDA, so the hidden routers are most likely in one of them"),
            "{}",
            claims[0].text
        );
        let (claims, _) = run_with(&Pdb::default(), &[("a", &silent)], &AsRel::from_rows([(100, 200, 0)]));
        assert!(claims[0].text.contains("The two peer directly per CAIDA"), "{}", claims[0].text);
    }
}
