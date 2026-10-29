//! PeeringDB, copied into the user's own database after they say yes, and read back for the map: which exchange an
//! address belongs to, whose port it is, and which buildings two networks both list.
//!
//! Nothing here is bundled or sent anywhere with the user's data in it: the requests are the public table listings
//! (`/api/<table>?depth=0`), sequential and four seconds apart, then only what changed since the last sync. Every row
//! keeps PeeringDB's own `updated` date so a claim can say how old its source is.

use crate::store::Store;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

const BASE: &str = "https://www.peeringdb.com/api";
/// The setting that says the user allowed the downloads.
pub(crate) const ACCEPTED: &str = "netmap.peeringdb_accepted";
/// How long between requests: PeeringDB asks anonymous clients to stay under 20 a minute.
pub(crate) const PAUSE: Duration = Duration::from_secs(4);

/// Each table with its columns as `(column, key in PeeringDB's JSON)`. A listing row has no `ix_id` for an exchange
/// prefix, only the id of its LAN (`ixlan_id`), which PeeringDB numbers one to one with the exchange; the facility
/// link names the network `local_asn`.
const TABLES: [(&str, &[(&str, &str)]); 5] = [
    ("ix", &[("name", "name"), ("name_long", "name_long"), ("city", "city"), ("country", "country"), ("updated", "updated")]),
    ("ixpfx", &[("ix_id", "ixlan_id"), ("prefix", "prefix"), ("updated", "updated")]),
    (
        "netixlan",
        &[("ix_id", "ix_id"), ("asn", "asn"), ("ipaddr4", "ipaddr4"), ("ipaddr6", "ipaddr6"), ("speed", "speed"), ("updated", "updated")],
    ),
    ("fac", &[("name", "name"), ("address1", "address1"), ("city", "city"), ("country", "country"), ("updated", "updated")]),
    ("netfac", &[("fac_id", "fac_id"), ("asn", "local_asn"), ("updated", "updated")]),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Ix {
    pub id: i64,
    pub name: String,
    pub name_long: String,
    pub city: String,
    pub country: String,
    pub updated: String,
}

/// An exchange's peering LAN prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct Ixpfx {
    pub id: i64,
    pub ix_id: i64,
    pub prefix: String,
    pub updated: String,
    net: IpAddr,
    len: u8,
}

/// A network's port on an exchange.
#[derive(Clone, Debug, PartialEq)]
pub struct Netixlan {
    pub id: i64,
    pub ix_id: i64,
    pub asn: u32,
    pub addr: IpAddr,
    pub speed: Option<i64>,
    pub updated: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fac {
    pub id: i64,
    pub name: String,
    pub address1: String,
    pub city: String,
    pub country: String,
    pub updated: String,
}

/// The synced tables, loaded once per run.
#[derive(Clone, Debug, Default)]
pub struct Pdb {
    pub synced_at: Option<i64>,
    pub rows: usize,
    ix: HashMap<i64, Ix>,
    lans: Vec<Ixpfx>,
    ports: HashMap<IpAddr, Netixlan>,
    facs: HashMap<i64, Fac>,
    /// Network to the buildings it lists, with the date of each listing.
    netfac: HashMap<u32, Vec<(i64, String)>>,
}

/// A prefix of either family as its masked network and length.
fn prefix_of(s: &str) -> Option<(IpAddr, u8)> {
    let (addr, len) = s.split_once('/')?;
    let len: u8 = len.parse().ok()?;
    Some(match addr.parse::<IpAddr>().ok()?.to_canonical() {
        IpAddr::V4(a) if len <= 32 => (IpAddr::V4(Ipv4Addr::from(u32::from(a) & v4_mask(len))), len),
        IpAddr::V6(a) if len <= 128 => (IpAddr::V6(Ipv6Addr::from(u128::from(a) & v6_mask(len))), len),
        _ => return None,
    })
}

fn v4_mask(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - len) }
}

fn v6_mask(len: u8) -> u128 {
    if len == 0 { 0 } else { u128::MAX << (128 - len) }
}

impl Pdb {
    pub fn load(store: &Store) -> Pdb {
        store.with_conn(|c| Pdb::read(c).unwrap_or_default())
    }

    /// When the tables were last synced and how many rows they hold, without loading them.
    pub fn stats(store: &Store) -> (Option<i64>, usize) {
        store.with_conn(|c| {
            // The oldest table's time, and none until every table has synced once: a sync that stopped halfway is
            // taken up again at the next refresh, not counted fresh.
            let synced = c
                .query_row("SELECT MIN(synced_at), COUNT(*) FROM pdb_sync", [], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)))
                .ok()
                .and_then(|(t, n)| if n as usize == TABLES.len() { t } else { None });
            let rows = TABLES
                .iter()
                .map(|(t, _)| c.query_row(&format!("SELECT COUNT(*) FROM pdb_{t}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize)
                .sum();
            (synced, rows)
        })
    }

    fn read(c: &Connection) -> rusqlite::Result<Pdb> {
        let text = |r: &rusqlite::Row<'_>, i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
        let mut p = Pdb { synced_at: c.query_row("SELECT MAX(synced_at) FROM pdb_sync", [], |r| r.get(0))?, ..Pdb::default() };
        for (table, _) in TABLES {
            p.rows += c.query_row(&format!("SELECT COUNT(*) FROM pdb_{table}"), [], |r| r.get::<_, i64>(0))? as usize;
        }
        for ix in c.prepare("SELECT id, name, name_long, city, country, updated FROM pdb_ix")?.query_map([], |r| {
            Ok(Ix {
                id: r.get(0)?,
                name: text(r, 1)?,
                name_long: text(r, 2)?,
                city: text(r, 3)?,
                country: text(r, 4)?,
                updated: text(r, 5)?,
            })
        })? {
            let ix = ix?;
            p.ix.insert(ix.id, ix);
        }
        for row in c
            .prepare("SELECT id, ix_id, prefix, updated FROM pdb_ixpfx")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, text(r, 2)?, text(r, 3)?)))?
        {
            let (id, ix_id, prefix, updated) = row?;
            if let (Some(ix_id), Some((net, len))) = (ix_id, prefix_of(&prefix)) {
                p.lans.push(Ixpfx { id, ix_id, prefix, updated, net, len });
            }
        }
        for row in c
            .prepare(
                "SELECT id, ix_id, asn, ipaddr4, ipaddr6, speed, updated FROM pdb_netixlan WHERE ipaddr4 IS NOT NULL OR ipaddr6 IS NOT NULL",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<u32>>(2)?,
                    [text(r, 3)?, text(r, 4)?],
                    r.get(5)?,
                    text(r, 6)?,
                ))
            })?
        {
            let (id, ix_id, asn, ips, speed, updated) = row?;
            let (Some(ix_id), Some(asn)) = (ix_id, asn) else { continue };
            for addr in ips.iter().filter_map(|ip| ip.parse::<IpAddr>().ok()) {
                p.ports.insert(addr.to_canonical(), Netixlan { id, ix_id, asn, addr, speed, updated: updated.clone() });
            }
        }
        for f in c.prepare("SELECT id, name, address1, city, country, updated FROM pdb_fac")?.query_map([], |r| {
            Ok(Fac {
                id: r.get(0)?,
                name: text(r, 1)?,
                address1: text(r, 2)?,
                city: text(r, 3)?,
                country: text(r, 4)?,
                updated: text(r, 5)?,
            })
        })? {
            let f = f?;
            p.facs.insert(f.id, f);
        }
        for row in c
            .prepare("SELECT fac_id, asn, updated FROM pdb_netfac")?
            .query_map([], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<u32>>(1)?, text(r, 2)?)))?
        {
            if let (Some(fac), Some(asn), updated) = row? {
                p.netfac.entry(asn).or_default().push((fac, updated));
            }
        }
        Ok(p)
    }

    pub fn ix(&self, id: i64) -> Option<&Ix> {
        self.ix.get(&id)
    }

    /// The port that has exactly this address.
    pub fn port_of(&self, addr: IpAddr) -> Option<&Netixlan> {
        self.ports.get(&addr.to_canonical())
    }

    /// The exchange LAN whose prefix holds this address (the longest, if two do).
    pub fn lan_of(&self, addr: IpAddr) -> Option<&Ixpfx> {
        let addr = addr.to_canonical();
        self.lans
            .iter()
            .filter(|l| match (addr, l.net) {
                (IpAddr::V4(a), IpAddr::V4(n)) => u32::from(a) & v4_mask(l.len) == u32::from(n),
                (IpAddr::V6(a), IpAddr::V6(n)) => u128::from(a) & v6_mask(l.len) == u128::from(n),
                _ => false,
            })
            .max_by_key(|l| l.len)
    }

    /// The buildings both networks list, by name, each with the `updated` date of each network's listing.
    pub fn shared_facilities(&self, asn_a: u32, asn_b: u32) -> Vec<(Fac, String, String)> {
        let (Some(a), Some(b)) = (self.netfac.get(&asn_a), self.netfac.get(&asn_b)) else { return Vec::new() };
        let mut out: Vec<(Fac, String, String)> = a
            .iter()
            .filter_map(|(fac, ua)| {
                let (_, ub) = b.iter().find(|(f, _)| f == fac)?;
                Some((self.facs.get(fac)?.clone(), ua.clone(), ub.clone()))
            })
            .collect();
        out.sort_by(|x, y| x.0.name.cmp(&y.0.name));
        out
    }
}

/// Tables built by hand, for the tests of the code that reads them.
#[cfg(test)]
impl Pdb {
    pub(crate) fn add_ix(&mut self, id: i64, name: &str) {
        self.ix.insert(
            id,
            Ix {
                id,
                name: name.into(),
                name_long: name.into(),
                city: "Ashburn".into(),
                country: "US".into(),
                updated: "2026-03-01T10:00:00Z".into(),
            },
        );
        self.synced_at = Some(1_790_000_000_000);
    }

    pub(crate) fn add_lan(&mut self, ix_id: i64, prefix: &str) {
        let (net, len) = prefix_of(prefix).unwrap();
        self.lans.push(Ixpfx { id: 5, ix_id, prefix: prefix.into(), updated: "2026-02-01T00:00:00Z".into(), net, len });
    }

    pub(crate) fn add_port(&mut self, ix_id: i64, asn: u32, ip: &str) {
        let addr: IpAddr = ip.parse().unwrap();
        self.ports.insert(addr, Netixlan { id: 100, ix_id, asn, addr, speed: Some(10_000), updated: "2026-04-02T08:30:00Z".into() });
    }

    /// `asn` lists the building `fac`, edited on `updated`.
    pub(crate) fn add_listing(&mut self, fac: i64, name: &str, asn: u32, updated: &str) {
        self.facs.entry(fac).or_insert_with(|| Fac {
            id: fac,
            name: name.into(),
            address1: String::new(),
            city: "Ashburn".into(),
            country: "US".into(),
            updated: String::new(),
        });
        self.netfac.entry(asn).or_default().push((fac, updated.into()));
        self.synced_at = Some(1_790_000_000_000);
    }
}

/// A JSON value as the database stores it.
fn sql(v: Option<&Value>) -> Sql {
    match v {
        Some(Value::String(s)) => Sql::Text(s.clone()),
        Some(Value::Bool(b)) => Sql::Integer(*b as i64),
        Some(Value::Number(n)) => n.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or_default())),
        _ => Sql::Null,
    }
}

/// Apply one table's rows in one transaction: a row whose status isn't `ok` (PeeringDB sends deleted rows with a
/// `since` request) is removed, the rest are written over what was there. Then note when and from where it was synced.
fn apply(c: &mut Connection, table: &str, cols: &[(&str, &str)], rows: &[Value], since: i64, now: i64) -> rusqlite::Result<()> {
    let tx = c.transaction()?;
    {
        let names: Vec<&str> = cols.iter().map(|(c, _)| *c).collect();
        let marks: Vec<String> = (2..names.len() + 2).map(|i| format!("?{i}")).collect();
        let mut put =
            tx.prepare(&format!("INSERT OR REPLACE INTO pdb_{table}(id, {}) VALUES(?1, {})", names.join(", "), marks.join(", ")))?;
        let mut del = tx.prepare(&format!("DELETE FROM pdb_{table} WHERE id = ?1"))?;
        for row in rows {
            let Some(id) = row["id"].as_i64() else { continue };
            if row["status"].as_str().is_some_and(|s| s != "ok") {
                del.execute(params![id])?;
            } else {
                let mut args = vec![Sql::Integer(id)];
                args.extend(cols.iter().map(|(_, key)| sql(row.get(*key))));
                put.execute(rusqlite::params_from_iter(args))?;
            }
        }
    }
    tx.execute("INSERT OR REPLACE INTO pdb_sync(\"table\", synced_at, since) VALUES(?1, ?2, ?3)", params![table, now, since.to_string()])?;
    tx.commit()
}

/// Copy the five tables, a full listing the first time and `since` the last sync after that. `fetch` gets a URL and
/// returns the body; it is where the caller refuses to fetch while a measurement runs. Only after the user allowed it.
pub async fn sync<F, Fut>(store: &Store, fetch: F, pause: Duration) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    if store.get::<bool>(ACCEPTED) != Some(true) {
        return Err("PeeringDB downloads are off.".into());
    }
    for (i, (table, cols)) in TABLES.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(pause).await;
        }
        let since: Option<String> = store.with_conn(|c| {
            c.query_row("SELECT since FROM pdb_sync WHERE \"table\" = ?1", params![table], |r| r.get::<_, Option<String>>(0))
                .optional()
                .ok()
                .flatten()
                .flatten()
        });
        // Asked at, not answered at: a row edited while the listing downloads is picked up next time.
        let asked = crate::bench::now_ms() / 1000;
        let url = match &since {
            Some(s) => format!("{BASE}/{table}?depth=0&since={s}"),
            None => format!("{BASE}/{table}?depth=0"),
        };
        let body = fetch(url).await?;
        let listing: Value = serde_json::from_slice(&body).map_err(|e| format!("PeeringDB {table}: {e}"))?;
        let rows = listing["data"].as_array().ok_or_else(|| format!("PeeringDB {table}: no data"))?;
        store.with_conn(|c| apply(c, table, cols, rows, asked, crate::bench::now_ms())).map_err(|e| format!("PeeringDB {table}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn scratch() -> (Store, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("recursa-pdb-{:x}.db", fastrand::u64(..)));
        (Store::open(path.clone()).unwrap(), path)
    }

    fn remove(path: &std::path::Path) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    /// A small hand-written listing in PeeringDB's shape (not a recording of the service).
    fn fixture(table: &str, second: bool) -> &'static str {
        match (table, second) {
            ("ix", _) => {
                r#"{"data":[{"id":31,"name":"Equinix Ashburn","name_long":"Equinix Internet Exchange Ashburn","city":"Ashburn","country":"US","updated":"2026-03-01T10:00:00Z","status":"ok"}]}"#
            }
            ("ixpfx", _) => {
                r#"{"data":[{"id":5,"ixlan_id":31,"protocol":"IPv4","prefix":"206.126.236.0/22","updated":"2026-02-01T00:00:00Z","status":"ok"},
                    {"id":6,"ixlan_id":31,"protocol":"IPv6","prefix":"2001:504:0:2::/64","updated":"2026-02-01T00:00:00Z","status":"ok"}]}"#
            }
            ("netixlan", false) => {
                r#"{"data":[{"id":100,"ix_id":31,"asn":46887,"ipaddr4":"206.126.237.148","ipaddr6":null,"speed":10000,"updated":"2026-04-02T08:30:00Z","status":"ok"},
                    {"id":101,"ix_id":31,"asn":13335,"ipaddr4":"206.126.236.20","ipaddr6":"2001:504:0:2::13","speed":100000,"updated":"2026-04-03T08:30:00Z","status":"ok"},
                    {"id":102,"ix_id":31,"asn":15169,"ipaddr4":"206.126.236.99","ipaddr6":null,"speed":100000,"updated":"2026-04-03T08:30:00Z","status":"ok"}]}"#
            }
            ("netixlan", true) => r#"{"data":[{"id":102,"status":"deleted","updated":"2026-09-01T00:00:00Z"}]}"#,
            ("fac", _) => {
                r#"{"data":[{"id":1,"name":"Equinix DC2","address1":"21715 Filigree Ct","city":"Ashburn","country":"US","updated":"2025-11-01T00:00:00Z","status":"ok"},
                    {"id":2,"name":"CoreSite VA1","address1":"1 Somewhere","city":"Reston","country":"US","updated":"2025-12-01T00:00:00Z","status":"ok"}]}"#
            }
            ("netfac", _) => {
                r#"{"data":[{"id":1,"fac_id":1,"local_asn":46887,"updated":"2026-01-10T00:00:00Z","status":"ok"},
                    {"id":2,"fac_id":1,"local_asn":13335,"updated":"2026-01-20T00:00:00Z","status":"ok"},
                    {"id":3,"fac_id":2,"local_asn":13335,"updated":"2026-01-21T00:00:00Z","status":"ok"}]}"#
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn sync_drops_deleted_rows_and_lookups_read_the_rest() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (store, path) = scratch();
        let urls = Mutex::new(Vec::<String>::new());
        let serve = |second: bool| {
            let urls = &urls;
            move |url: String| async move {
                let table = url.split("/api/").nth(1).unwrap().split('?').next().unwrap().to_string();
                urls.lock().unwrap().push(url);
                Ok(fixture(&table, second).as_bytes().to_vec())
            }
        };
        // Nothing is fetched before the user allows it.
        assert!(rt.block_on(sync(&store, serve(false), Duration::ZERO)).is_err() && urls.lock().unwrap().is_empty());
        store.put(ACCEPTED, &true);
        rt.block_on(sync(&store, serve(false), Duration::ZERO)).unwrap();
        assert!(urls.lock().unwrap().iter().all(|u| u.starts_with("https://www.peeringdb.com/api/") && u.ends_with("?depth=0")));
        let first = Pdb::load(&store);
        assert_eq!(first.ports.len(), 4);
        assert!(first.synced_at.is_some());

        // The second sync asks `since` the first, and PeeringDB sends the deleted row back with its status.
        urls.lock().unwrap().clear();
        rt.block_on(sync(&store, serve(true), Duration::ZERO)).unwrap();
        assert!(urls.lock().unwrap().iter().all(|u| u.contains("?depth=0&since=")));
        let p = Pdb::load(&store);
        assert_eq!(p.ports.len(), 3);
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(p.port_of(ip("206.126.236.99")).is_none());
        // `port_of` is an exact match.
        let port = p.port_of(ip("206.126.237.148")).unwrap();
        assert_eq!((port.asn, port.ix_id, port.speed), (46887, 31, Some(10000)));
        assert!(p.port_of(ip("206.126.237.149")).is_none());
        assert_eq!(p.port_of(ip("2001:504:0:2::13")).map(|n| (n.asn, n.ix_id)), Some((13335, 31)));
        // `lan_of` is a prefix match, per family.
        assert_eq!(p.lan_of(ip("206.126.238.7")).map(|l| (l.id, l.ix_id)), Some((5, 31)));
        assert!(p.lan_of(ip("206.126.240.1")).is_none() && p.lan_of(ip("2001:504:0:3::1")).is_none());
        assert_eq!(p.lan_of(ip("2001:504:0:2::1")).map(|l| (l.id, l.ix_id)), Some((6, 31)));
        assert_eq!(p.ix(31).unwrap().name, "Equinix Ashburn");
        // Both networks list one building; each side's `updated` comes back.
        let shared = p.shared_facilities(46887, 13335);
        assert_eq!(shared.len(), 1);
        assert_eq!(
            (shared[0].0.name.as_str(), shared[0].1.as_str(), shared[0].2.as_str()),
            ("Equinix DC2", "2026-01-10T00:00:00Z", "2026-01-20T00:00:00Z")
        );
        assert!(p.shared_facilities(46887, 15169).is_empty());
        drop(store);
        remove(&path);
    }
}
