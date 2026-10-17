//! The compact VRP table kept after a sync, and RFC 6811 route origin validation against it.
//!
//! Entries are sorted by (prefix length, address), one list per address family, so the VRPs that
//! cover a route are found with one binary search per shorter-or-equal prefix length.

use std::net::IpAddr;

/// One validated ROA payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Vrp {
    pub addr: IpAddr,
    pub len: u8,
    pub max_len: u8,
    pub asn: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Entry<A: Ord> {
    len: u8,
    addr: A,
    max_len: u8,
    asn: u32,
}

type E4 = Entry<[u8; 4]>;
type E6 = Entry<[u8; 16]>;

#[derive(Default)]
pub(super) struct Table {
    v4: Vec<E4>,
    v6: Vec<E6>,
}

/// How one VRP relates to a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Match {
    /// Same origin AS and the route is no longer than the maximum length.
    Valid,
    /// Same origin AS, but the route is longer than the maximum length.
    TooLong,
    /// Another origin AS.
    OtherAs,
}

impl Match {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Match::Valid => "valid",
            Match::TooLong => "invalid_length",
            Match::OtherAs => "invalid_asn",
        }
    }
}

/// The RFC 6811 state of a route: the covering VRPs, best match first, and the verdict.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Validity {
    /// `valid`, `invalid_asn`, `invalid_length`, or `unknown` (no VRP covers the route: "not found").
    pub status: &'static str,
    pub covering: Vec<(Vrp, Match)>,
}

fn mask<const N: usize>(addr: [u8; N], len: u8) -> [u8; N] {
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        let bits = usize::from(len).saturating_sub(i * 8).min(8);
        *b = if bits == 0 { 0 } else { addr[i] & (0xffu8 << (8 - bits)) };
    }
    out
}

/// The entries of `list` with exactly this prefix length and address.
fn exact<A: Ord + Copy>(list: &[Entry<A>], len: u8, addr: A) -> &[Entry<A>] {
    let lo = list.partition_point(|e| (e.len, e.addr) < (len, addr));
    let hi = lo + list[lo..].partition_point(|e| (e.len, e.addr) == (len, addr));
    &list[lo..hi]
}

/// The VRPs of `list` that cover `addr/len`: one lookup per prefix length up to `len`.
fn gather<const N: usize>(list: &[Entry<[u8; N]>], addr: [u8; N], len: u8) -> Vec<Vrp>
where
    IpAddr: From<[u8; N]>,
{
    let mut out = Vec::new();
    for l in 0..=len {
        out.extend(exact(list, l, mask(addr, l)).iter().map(|e| Vrp {
            addr: IpAddr::from(e.addr),
            len: e.len,
            max_len: e.max_len,
            asn: e.asn,
        }));
    }
    out
}

impl Table {
    pub(super) fn new(vrps: impl IntoIterator<Item = Vrp>) -> Self {
        let mut t = Table::default();
        for v in vrps {
            match v.addr {
                IpAddr::V4(a) if v.len <= 32 => {
                    t.v4.push(Entry { len: v.len, addr: mask(a.octets(), v.len), max_len: v.max_len, asn: v.asn })
                }
                IpAddr::V6(a) if v.len <= 128 => {
                    t.v6.push(Entry { len: v.len, addr: mask(a.octets(), v.len), max_len: v.max_len, asn: v.asn })
                }
                _ => {}
            }
        }
        t.v4.sort_unstable();
        t.v4.dedup();
        t.v4.shrink_to_fit();
        t.v6.sort_unstable();
        t.v6.dedup();
        t.v6.shrink_to_fit();
        t
    }

    pub(super) fn len(&self) -> usize {
        self.v4.len() + self.v6.len()
    }

    /// Every VRP, in table order.
    pub(super) fn iter(&self) -> impl Iterator<Item = Vrp> + '_ {
        let v4 = self.v4.iter().map(|e| Vrp { addr: IpAddr::from(e.addr), len: e.len, max_len: e.max_len, asn: e.asn });
        let v6 = self.v6.iter().map(|e| Vrp { addr: IpAddr::from(e.addr), len: e.len, max_len: e.max_len, asn: e.asn });
        v4.chain(v6)
    }

    /// Validates the route `addr/len` announced by `origin`.
    pub(super) fn validate(&self, addr: IpAddr, len: u8, origin: u32) -> Validity {
        let found = match addr {
            IpAddr::V4(a) if len <= 32 => gather(&self.v4, a.octets(), len),
            IpAddr::V6(a) if len <= 128 => gather(&self.v6, a.octets(), len),
            _ => Vec::new(),
        };
        let mut covering: Vec<(Vrp, Match)> = found
            .into_iter()
            .map(|v| {
                let m = if v.asn != origin || v.asn == 0 {
                    Match::OtherAs
                } else if len > v.max_len {
                    Match::TooLong
                } else {
                    Match::Valid
                };
                (v, m)
            })
            .collect();
        // Valid first, then the same AS with a length problem, then the rest; longer prefixes first.
        covering.sort_by_key(|(v, m)| (*m, std::cmp::Reverse(v.len), v.asn));
        let status = match covering.first() {
            None => "unknown",
            Some((_, m)) => m.as_str(),
        };
        Validity { status, covering }
    }
}

const MAGIC: &[u8; 8] = b"RECVRP01";

impl Table {
    /// The table as saved on disk: a header with the time of the sync (ms since the epoch), then one record per VRP.
    pub(super) fn to_bytes(&self, time: i64) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + self.v4.len() * 10 + self.v6.len() * 22);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.len()).unwrap_or(u32::MAX).to_le_bytes());
        for v in self.iter() {
            out.push(v.len);
            out.push(v.max_len);
            match v.addr {
                IpAddr::V4(a) => {
                    out.push(4);
                    out.extend_from_slice(&a.octets());
                }
                IpAddr::V6(a) => {
                    out.push(6);
                    out.extend_from_slice(&a.octets());
                }
            }
            out.extend_from_slice(&v.asn.to_le_bytes());
        }
        out
    }

    /// The inverse of [`Table::to_bytes`]; `None` for anything that isn't a whole, well-formed table.
    pub(super) fn from_bytes(b: &[u8]) -> Option<(Table, i64)> {
        let rest = b.strip_prefix(MAGIC)?;
        let time = i64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
        let count = u32::from_le_bytes(rest.get(8..12)?.try_into().ok()?) as usize;
        let mut rest = &rest[12..];
        // The smallest record is an IPv4 one: length, max length, family, 4 address bytes, 4 for the AS.
        if count > rest.len() / 11 {
            return None;
        }
        let mut vrps = Vec::with_capacity(count);
        for _ in 0..count {
            let (&len, r) = rest.split_first()?;
            let (&max_len, r) = r.split_first()?;
            let (&fam, r) = r.split_first()?;
            let (addr, r) = match fam {
                4 => (IpAddr::from(<[u8; 4]>::try_from(r.get(..4)?).ok()?), r.get(4..)?),
                6 => (IpAddr::from(<[u8; 16]>::try_from(r.get(..16)?).ok()?), r.get(16..)?),
                _ => return None,
            };
            let asn = u32::from_le_bytes(r.get(..4)?.try_into().ok()?);
            rest = r.get(4..)?;
            vrps.push(Vrp { addr, len, max_len, asn });
        }
        rest.is_empty().then(|| (Table::new(vrps), time))
    }
}
