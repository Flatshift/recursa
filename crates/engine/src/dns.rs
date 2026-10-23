//! Minimal DNS wire format: query encoding and response decoding.
//!
//! Covers what a resolver benchmark needs: RFC 1035 messages and name compression,
//! EDNS(0) (RFC 6891) with NSID (5001), Client Subnet (7871), Cookies (7873) and
//! Extended DNS Errors (8914). Queries are encoded once and only the ID is patched
//! on the hot path.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};

pub const TYPE_A: u16 = 1;
pub const TYPE_NS: u16 = 2;
const TYPE_CNAME: u16 = 5;
pub const TYPE_SOA: u16 = 6;
const TYPE_PTR: u16 = 12;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_NSEC: u16 = 47;
pub const TYPE_DNSKEY: u16 = 48;
pub const TYPE_NSEC3: u16 = 50;
pub const TYPE_OPT: u16 = 41;

const CLASS_IN: u16 = 1;
const CLASS_CH: u16 = 3;

pub const OPT_NSID: u16 = 3;
pub const OPT_COOKIE: u16 = 10;
const OPT_EDE: u16 = 15;

pub const RCODE_NOERROR: u16 = 0;
pub const RCODE_SERVFAIL: u16 = 2;
pub const RCODE_NXDOMAIN: u16 = 3;
pub const RCODE_REFUSED: u16 = 5;
pub const RCODE_BADVERS: u16 = 16;

const FLAG_AA: u16 = 0x0400;
const FLAG_TC: u16 = 0x0200;
const FLAG_RD: u16 = 0x0100;
const FLAG_AD: u16 = 0x0020;
const FLAG_CD: u16 = 0x0010;

#[derive(Clone, Debug, Default)]
pub struct Edns {
    pub udp_size: u16,
    pub version: u8,
    pub dnssec_ok: bool,
    pub nsid: bool,
    pub cookie: Option<[u8; 8]>,
}

impl Edns {
    pub fn standard() -> Self {
        Edns { udp_size: 1232, version: 0, dnssec_ok: false, nsid: false, cookie: None }
    }
}

#[derive(Clone, Debug)]
pub struct QuerySpec {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    pub rd: bool,
    pub cd: bool,
    pub edns: Option<Edns>,
}

impl QuerySpec {
    pub fn new(name: impl Into<String>, qtype: u16) -> Self {
        QuerySpec { name: name.into(), qtype, qclass: CLASS_IN, rd: true, cd: false, edns: Some(Edns::standard()) }
    }
    pub fn chaos_txt(name: &str) -> Self {
        QuerySpec { name: name.into(), qtype: TYPE_TXT, qclass: CLASS_CH, rd: false, cd: false, edns: Some(Edns::standard()) }
    }
    pub fn with_edns(mut self, f: impl FnOnce(&mut Edns)) -> Self {
        let mut e = self.edns.take().unwrap_or_else(Edns::standard);
        f(&mut e);
        self.edns = Some(e);
        self
    }
}

#[derive(Debug)]
pub struct EncodeError(pub &'static str);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

fn encode_name(out: &mut Vec<u8>, name: &str) -> Result<(), EncodeError> {
    let name = name.trim_end_matches('.');
    let start = out.len();
    if !name.is_empty() {
        for label in name.split('.') {
            let b = label.as_bytes();
            if b.is_empty() {
                return Err(EncodeError("empty label"));
            }
            if b.len() > 63 {
                return Err(EncodeError("label longer than 63 octets"));
            }
            out.push(b.len() as u8);
            out.extend_from_slice(b);
        }
    }
    out.push(0);
    if out.len() - start > 255 {
        return Err(EncodeError("name longer than 255 octets"));
    }
    Ok(())
}

/// A query ID nobody off the path can guess, so a spoofed reply has 1 in 65,536 chances on top of matching the port.
pub fn new_id() -> u16 {
    u16::from_ne_bytes(crate::unpredictable())
}

/// Encode a complete query message. The ID can later be patched with [`set_id`].
pub fn encode_query(id: u16, q: &QuerySpec) -> Result<Vec<u8>, EncodeError> {
    let mut m = Vec::with_capacity(64 + q.name.len());
    let mut flags = 0u16;
    if q.rd {
        flags |= FLAG_RD;
    }
    if q.cd {
        flags |= FLAG_CD;
    }
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&flags.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    m.extend_from_slice(&0u16.to_be_bytes()); // ancount
    m.extend_from_slice(&0u16.to_be_bytes()); // nscount
    m.extend_from_slice(&(q.edns.is_some() as u16).to_be_bytes()); // arcount
    encode_name(&mut m, &q.name)?;
    m.extend_from_slice(&q.qtype.to_be_bytes());
    m.extend_from_slice(&q.qclass.to_be_bytes());
    if let Some(e) = &q.edns {
        m.push(0); // root owner
        m.extend_from_slice(&TYPE_OPT.to_be_bytes());
        m.extend_from_slice(&e.udp_size.to_be_bytes());
        m.push(0); // extended rcode
        m.push(e.version);
        let z: u16 = if e.dnssec_ok { 0x8000 } else { 0 };
        m.extend_from_slice(&z.to_be_bytes());
        let mut opts = Vec::new();
        if e.nsid {
            opts.extend_from_slice(&OPT_NSID.to_be_bytes());
            opts.extend_from_slice(&0u16.to_be_bytes());
        }
        if let Some(c) = e.cookie {
            opts.extend_from_slice(&OPT_COOKIE.to_be_bytes());
            opts.extend_from_slice(&8u16.to_be_bytes());
            opts.extend_from_slice(&c);
        }
        m.extend_from_slice(&(opts.len() as u16).to_be_bytes());
        m.extend_from_slice(&opts);
    }
    Ok(m)
}

#[inline]
pub fn set_id(msg: &mut [u8], id: u16) {
    msg[0..2].copy_from_slice(&id.to_be_bytes());
}

/// Cheap check that a datagram is a response to the query we sent: QR set, same ID
/// and an identical question section (compared case-insensitively).
pub fn is_response_to(resp: &[u8], query: &[u8]) -> bool {
    if resp.len() < 12 || query.len() < 12 {
        return false;
    }
    if resp[0..2] != query[0..2] || resp[2] & 0x80 == 0 {
        return false;
    }
    let qd = u16::from_be_bytes([resp[4], resp[5]]);
    if qd == 0 {
        // Some servers omit the question on errors (e.g. FORMERR); accept on ID match.
        return true;
    }
    let qlen = question_len(query);
    match qlen {
        Some(len) if resp.len() >= 12 + len => resp[12..12 + len].eq_ignore_ascii_case(&query[12..12 + len]),
        _ => false,
    }
}

fn question_len(msg: &[u8]) -> Option<usize> {
    let mut i = 12;
    loop {
        let l = *msg.get(i)? as usize;
        if l == 0 {
            i += 1;
            break;
        }
        if l & 0xC0 != 0 {
            return None;
        }
        i += 1 + l;
    }
    Some(i + 4 - 12)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Name(String),
    Txt(Vec<String>),
    Soa { mname: String, minimum: u32 },
    Other(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Record {
    pub name: String,
    pub rtype: u16,
    pub ttl: u32,
    pub data: RData,
}

#[derive(Clone, Debug, Default)]
pub struct OptRecord {
    pub udp_size: u16,
    pub version: u8,
    pub dnssec_ok: bool,
    pub options: Vec<(u16, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub flags: u16,
    /// Full rcode including the EDNS extended bits.
    pub rcode: u16,
    pub qname: String,
    pub answers: Vec<Record>,
    pub authority: Vec<Record>,
    pub opt: Option<OptRecord>,
}

#[derive(Debug)]
pub struct ParseError(pub &'static str);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, ParseError> {
        let v = *self.b.get(self.pos).ok_or(ParseError("truncated"))?;
        self.pos += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, ParseError> {
        Ok(((self.u8()? as u16) << 8) | self.u8()? as u16)
    }
    fn u32(&mut self) -> Result<u32, ParseError> {
        Ok(((self.u16()? as u32) << 16) | self.u16()? as u32)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(n).ok_or(ParseError("overflow"))?;
        let s = self.b.get(self.pos..end).ok_or(ParseError("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    fn name(&mut self) -> Result<String, ParseError> {
        let (n, next) = read_name(self.b, self.pos)?;
        self.pos = next;
        Ok(n)
    }
}

/// Decode a possibly-compressed name at `pos`. Returns the name and the offset just
/// past it in the original (uncompressed) position.
fn read_name(b: &[u8], mut pos: usize) -> Result<(String, usize), ParseError> {
    let mut out = String::new();
    let mut jumped = false;
    let mut next = 0usize;
    let mut hops = 0;
    loop {
        let l = *b.get(pos).ok_or(ParseError("truncated name"))? as usize;
        if l & 0xC0 == 0xC0 {
            let lo = *b.get(pos + 1).ok_or(ParseError("truncated pointer"))? as usize;
            if !jumped {
                next = pos + 2;
            }
            pos = ((l & 0x3F) << 8) | lo;
            jumped = true;
            hops += 1;
            if hops > 64 {
                return Err(ParseError("compression loop"));
            }
            continue;
        }
        if l & 0xC0 != 0 {
            return Err(ParseError("bad label type"));
        }
        if l == 0 {
            if !jumped {
                next = pos + 1;
            }
            break;
        }
        let label = b.get(pos + 1..pos + 1 + l).ok_or(ParseError("truncated label"))?;
        if !out.is_empty() {
            out.push('.');
        }
        for &c in label {
            if c.is_ascii_graphic() && c != b'.' && c != b'\\' {
                out.push(c as char);
            } else {
                let _ = write!(out, "\\{:03}", c);
            }
        }
        if out.len() > 1024 {
            return Err(ParseError("name too long"));
        }
        pos += 1 + l;
    }
    if out.is_empty() {
        out.push('.');
    }
    Ok((out, next))
}

pub fn parse(b: &[u8]) -> Result<Message, ParseError> {
    let mut r = Reader { b, pos: 0 };
    let _id = r.u16()?;
    let flags = r.u16()?;
    let qd = r.u16()?;
    let an = r.u16()?;
    let ns = r.u16()?;
    let ar = r.u16()?;
    let mut qname = String::new();
    for i in 0..qd {
        let n = r.name()?;
        let _t = r.u16()?;
        let _c = r.u16()?;
        if i == 0 {
            qname = n;
        }
    }
    // A record takes at least 11 bytes, so the count can't ask for more room than the packet could fill.
    let mut answers = Vec::with_capacity((an as usize).min(b.len() / 11));
    let mut authority = Vec::new();
    let mut opt = None;
    let mut ext_rcode_hi: u16 = 0;
    for section in 0..3 {
        let count = [an, ns, ar][section];
        for _ in 0..count {
            let name = r.name()?;
            let rtype = r.u16()?;
            let class = r.u16()?;
            let ttl = r.u32()?;
            let rdlen = r.u16()? as usize;
            let rd_start = r.pos;
            let rdata = r.bytes(rdlen)?;
            if rtype == TYPE_OPT {
                ext_rcode_hi = (ttl >> 24) as u16;
                let mut o =
                    OptRecord { udp_size: class, version: ((ttl >> 16) & 0xFF) as u8, dnssec_ok: ttl & 0x8000 != 0, options: Vec::new() };
                let mut or = Reader { b: rdata, pos: 0 };
                while or.pos + 4 <= rdata.len() {
                    let code = or.u16()?;
                    let len = or.u16()? as usize;
                    o.options.push((code, or.bytes(len)?.to_vec()));
                }
                opt = Some(o);
                continue;
            }
            let data = match (rtype, rdlen) {
                (TYPE_A, 4) => RData::A(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3])),
                (TYPE_AAAA, 16) => {
                    let mut a = [0u8; 16];
                    a.copy_from_slice(rdata);
                    RData::Aaaa(Ipv6Addr::from(a))
                }
                (TYPE_CNAME | TYPE_NS | TYPE_PTR, _) => RData::Name(read_name(b, rd_start)?.0),
                (TYPE_TXT, _) => {
                    let mut parts = Vec::new();
                    let mut i = 0;
                    while i < rdata.len() {
                        let l = rdata[i] as usize;
                        let s = rdata.get(i + 1..i + 1 + l).ok_or(ParseError("truncated txt"))?;
                        parts.push(String::from_utf8_lossy(s).into_owned());
                        i += 1 + l;
                    }
                    RData::Txt(parts)
                }
                (TYPE_SOA, _) => {
                    let (mname, p) = read_name(b, rd_start)?;
                    let (_rname, p) = read_name(b, p)?;
                    let min = b.get(p + 16..p + 20).ok_or(ParseError("truncated soa"))?;
                    RData::Soa { mname, minimum: u32::from_be_bytes([min[0], min[1], min[2], min[3]]) }
                }
                _ => RData::Other(rdata.to_vec()),
            };
            let rec = Record { name, rtype, ttl, data };
            match section {
                0 => answers.push(rec),
                1 => authority.push(rec),
                _ => {}
            }
        }
    }
    let rcode = (ext_rcode_hi << 4) | (flags & 0x000F);
    Ok(Message { flags, rcode, qname, answers, authority, opt })
}

impl Message {
    pub fn ad(&self) -> bool {
        self.flags & FLAG_AD != 0
    }

    /// Authoritative Answer: the server owns the zone (no resolver or forwarder in between).
    pub fn aa(&self) -> bool {
        self.flags & FLAG_AA != 0
    }

    /// Truncated: the answer didn't fit and should be asked again over TCP.
    pub fn tc(&self) -> bool {
        self.flags & FLAG_TC != 0
    }

    pub fn option(&self, code: u16) -> Option<&[u8]> {
        self.opt.as_ref()?.options.iter().find(|(c, _)| *c == code).map(|(_, v)| v.as_slice())
    }

    /// Extended DNS Errors (RFC 8914): (info-code, extra-text).
    pub fn ede(&self) -> Vec<(u16, String)> {
        let Some(o) = &self.opt else { return Vec::new() };
        o.options
            .iter()
            .filter(|(c, v)| *c == OPT_EDE && v.len() >= 2)
            .map(|(_, v)| {
                let code = u16::from_be_bytes([v[0], v[1]]);
                let text = String::from_utf8_lossy(&v[2..]).trim_end_matches('\0').to_string();
                (code, text)
            })
            .collect()
    }

    /// NSID payload, rendered as text when printable, else hex.
    pub fn nsid(&self) -> Option<String> {
        let v = self.option(OPT_NSID)?;
        if v.is_empty() {
            return None;
        }
        if v.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            Some(String::from_utf8_lossy(v).into_owned())
        } else {
            Some(v.iter().map(|b| format!("{b:02x}")).collect())
        }
    }

    pub fn ipv4_answers(&self) -> Vec<Ipv4Addr> {
        self.answers.iter().filter_map(|r| if let RData::A(a) = r.data { Some(a) } else { None }).collect()
    }

    pub fn ipv6_answers(&self) -> Vec<Ipv6Addr> {
        self.answers.iter().filter_map(|r| if let RData::Aaaa(a) = r.data { Some(a) } else { None }).collect()
    }

    pub fn txt_answers(&self) -> Vec<String> {
        self.answers.iter().filter_map(|r| if let RData::Txt(t) = &r.data { Some(t.concat()) } else { None }).collect()
    }
}

/// The 32 nibbles of an IPv6 address, least significant first, dot-separated (the labels of `ip6.arpa`).
pub fn nibbles(a: Ipv6Addr) -> String {
    let mut out = String::with_capacity(63);
    for b in a.octets().iter().rev() {
        for n in [b & 0xf, b >> 4] {
            if !out.is_empty() {
                out.push('.');
            }
            out.push(char::from_digit(n as u32, 16).unwrap());
        }
    }
    out
}

pub fn rcode_name(rc: u16) -> &'static str {
    match rc {
        0 => "NOERROR",
        1 => "FORMERR",
        2 => "SERVFAIL",
        3 => "NXDOMAIN",
        4 => "NOTIMP",
        5 => "REFUSED",
        6 => "YXDOMAIN",
        9 => "NOTAUTH",
        16 => "BADVERS",
        23 => "BADCOOKIE",
        _ => "RCODE?",
    }
}

/// RFC 8914 registry names.
pub fn ede_name(code: u16) -> &'static str {
    match code {
        0 => "Other",
        1 => "Unsupported DNSKEY Algorithm",
        2 => "Unsupported DS Digest Type",
        3 => "Stale Answer",
        4 => "Forged Answer",
        5 => "DNSSEC Indeterminate",
        6 => "DNSSEC Bogus",
        7 => "Signature Expired",
        8 => "Signature Not Yet Valid",
        9 => "DNSKEY Missing",
        10 => "RRSIGs Missing",
        11 => "No Zone Key Bit Set",
        12 => "NSEC Missing",
        13 => "Cached Error",
        14 => "Not Ready",
        15 => "Blocked",
        16 => "Censored",
        17 => "Filtered",
        18 => "Prohibited",
        19 => "Stale NXDOMAIN Answer",
        20 => "Not Authoritative",
        21 => "Not Supported",
        22 => "No Reachable Authority",
        23 => "Network Error",
        24 => "Invalid Data",
        25 => "Signature Expired before Valid",
        26 => "Too Early",
        27 => "Unsupported NSEC3 Iterations Value",
        28 => "Unable to conform to policy",
        29 => "Synthesized",
        30 => "Invalid Query Type",
        _ => "Unassigned",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_query_header() {
        let q = QuerySpec::new("www.example.com", TYPE_A).with_edns(|e| e.nsid = true);
        let mut m = encode_query(0x1234, &q).unwrap();
        set_id(&mut m, 0xBEEF);
        assert_eq!(&m[0..2], &[0xBE, 0xEF]);
        let p = parse(&m).unwrap();
        assert_eq!(p.qname, "www.example.com");
        assert!(p.opt.is_some());
        assert_eq!(p.opt.unwrap().options[0].0, OPT_NSID);
    }

    #[test]
    fn rejects_long_label() {
        let long = "a".repeat(64);
        assert!(encode_query(1, &QuerySpec::new(long, TYPE_A)).is_err());
    }

    #[test]
    fn response_matching() {
        let q = encode_query(7, &QuerySpec::new("Example.COM", TYPE_A)).unwrap();
        let mut r = q.clone();
        r[2] |= 0x80;
        r[13..20].copy_from_slice(b"example");
        assert!(is_response_to(&r, &q));
        r[1] = 8;
        assert!(!is_response_to(&r, &q));
    }
}
