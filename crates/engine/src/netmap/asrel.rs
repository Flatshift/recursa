//! CAIDA's AS Relationships table (serial-2): which networks buy transit from which, and which peer.
//!
//! Fetched monthly, like the prefix table, and kept in the store. A map reads only the pairs that touch the networks
//! its traces crossed. The table is inferred from BGP, so a relationship here is a published inference, not a contract.

use super::BODY_MAX;
use crate::store::Store;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::sync::Arc;

pub const DIR_URL: &str = "https://publicdata.caida.org/datasets/as-relationships/serial-2/";
pub const FETCHED_AT: &str = "netmap.asrel_fetched_at";
/// The file name, e.g. `20261001.as-rel2.txt.bz2`.
pub const SOURCE: &str = "netmap.asrel_source";
pub const REFRESH_MS: i64 = 31 * 24 * 3600 * 1000;
const SUFFIX: &str = ".as-rel2.txt.bz2";

/// What one network is to another, from the first one's side.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rel {
    /// The first buys transit from the second.
    Transit,
    /// Settlement-free peers.
    Peer,
    /// The second is the first's customer.
    Customer,
}

/// Relationships for the networks one map touches, from the `from` side's point of view.
#[derive(Default)]
pub struct AsRel(HashMap<(u32, u32), Rel>);

impl AsRel {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Every stored pair with at least one network in `asns`, both ways round.
    pub fn for_asns(store: &Store, asns: &[u32]) -> Self {
        Self::from_rows(store.asrel_touching(asns))
    }

    pub(crate) fn from_rows(rows: impl IntoIterator<Item = (u32, u32, i8)>) -> Self {
        let mut m = HashMap::new();
        for (a, b, rel) in rows {
            match rel {
                -1 => {
                    m.insert((b, a), Rel::Transit);
                    m.insert((a, b), Rel::Customer);
                }
                0 => {
                    m.insert((a, b), Rel::Peer);
                    m.insert((b, a), Rel::Peer);
                }
                _ => {}
            }
        }
        Self(m)
    }

    pub fn get(&self, from: u32, to: u32) -> Option<Rel> {
        self.0.get(&(from, to)).copied()
    }
}

/// The newest file name in the directory listing, or None when nothing matches.
pub fn newest(listing: &str) -> Option<String> {
    listing
        .match_indices(SUFFIX)
        .filter_map(|(i, _)| listing.get(i.checked_sub(8)?..i))
        .filter(|d| d.bytes().all(|b| b.is_ascii_digit()))
        .max()
        .map(|d| format!("{d}{SUFFIX}"))
}

/// Rows from the unpacked text, in the file's own orientation: `(provider, customer, -1)` and `(peer, peer, 0)`.
/// Skips comments and lines that are not four columns with numeric networks.
pub fn parse(text: &str) -> Vec<(u32, u32, i8)> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut c = l.split('|');
            let (a, b, r) = (c.next()?, c.next()?, c.next()?);
            c.next()?;
            let rel = match r {
                "-1" => -1,
                "0" => 0,
                _ => return None,
            };
            Some((a.parse().ok()?, b.parse().ok()?, rel))
        })
        .collect()
}

/// "October 2026" from `20261001.as-rel2.txt.bz2`.
pub fn month_of(source: &str) -> Option<String> {
    const NAMES: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let year = source.get(..4).filter(|y| y.bytes().all(|b| b.is_ascii_digit()))?;
    let month: usize = source.get(4..6)?.parse().ok()?;
    Some(format!("{} {year}", NAMES.get(month.checked_sub(1)?)?))
}

/// Fetch the newest table and replace the stored one with it. `fetch` gets a URL and returns the body. The old table
/// stays if anything fails.
pub async fn refresh<F, Fut>(store: &Arc<Store>, fetch: F) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let listing = String::from_utf8_lossy(&fetch(DIR_URL.into()).await?).into_owned();
    let name = newest(&listing).ok_or("CAIDA's relationship list has no files.")?;
    let packed = fetch(format!("{DIR_URL}{name}")).await?;
    let s = store.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        bzip2::read::BzDecoder::new(&packed[..]).take(BODY_MAX as u64 + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
        if out.len() > BODY_MAX {
            return Err("CAIDA's relationship file is too big to be the table.".to_string());
        }
        let rows = parse(&String::from_utf8_lossy(&out));
        s.asrel_replace(&rows).map_err(|e| e.to_string())?;
        Ok(rows.len())
    })
    .await
    .map_err(|e| e.to_string())??;
    if rows == 0 {
        return Err("CAIDA's relationship table had no rows.".into());
    }
    store.put(FETCHED_AT, &crate::bench::now_ms());
    store.put(SOURCE, &name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_both_kinds_and_skips_the_rest() {
        let text = "# source:topology\n# more\n1299|46887|-1|bgp\n2906|46887|0|bgp\n1|2|-1\nx|2|-1|bgp\n3|y|0|bgp\n\n4|5|1|bgp\n";
        assert_eq!(parse(text), [(1299, 46887, -1), (2906, 46887, 0)]);
    }

    #[test]
    fn newest_picks_the_latest_matching_file() {
        let page = r#"<a href="20260901.as-rel2.txt.bz2">a</a> <a href="20261001.as-rel2.txt.bz2">b</a>
            <a href="20261101.as-rel.txt.bz2">c</a> <a href="20261201.ppdc-ases.txt.bz2">d</a> <a href="README.txt">r</a>"#;
        assert_eq!(newest(page).as_deref(), Some("20261001.as-rel2.txt.bz2"));
        assert_eq!(newest(""), None);
    }

    #[test]
    fn month_reads_the_file_name() {
        assert_eq!(month_of("20261001.as-rel2.txt.bz2").as_deref(), Some("October 2026"));
        assert_eq!(month_of("20261301.as-rel2.txt.bz2"), None);
        assert_eq!(month_of("junk"), None);
    }

    #[test]
    fn get_reads_from_the_right_side() {
        let rels = AsRel::from_rows([(1299, 46887, -1), (2906, 46887, 0)]);
        assert_eq!(rels.get(46887, 1299), Some(Rel::Transit));
        assert_eq!(rels.get(1299, 46887), Some(Rel::Customer));
        assert_eq!(rels.get(46887, 2906), Some(Rel::Peer));
        assert_eq!(rels.get(2906, 46887), Some(Rel::Peer));
        assert_eq!(rels.get(46887, 13335), None);
    }
}
