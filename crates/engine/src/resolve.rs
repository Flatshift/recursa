//! Plain DNS questions through the system resolvers, using the engine's own DNS code.

use crate::dns::{self, QuerySpec};
use crate::net::{self, Endpoint, Exchanger, Transport, UdpX};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

#[derive(Clone)]
pub struct Resolve {
    servers: Vec<IpAddr>,
}

impl Resolve {
    pub fn new(servers: Vec<IpAddr>) -> Self {
        Resolve { servers }
    }

    /// Ask the first system resolver that answers. Any answer counts, SERVFAIL and NXDOMAIN
    /// included: the caller decides what those mean. Two passes over the servers on timeouts.
    pub fn query(&self, name: &str, qtype: u16) -> Result<dns::Message, String> {
        if self.servers.is_empty() {
            return Err("no system resolver found".into());
        }
        let mut last = String::from("timed out");
        for _ in 0..2 {
            for s in &self.servers {
                let attempt = || -> Result<dns::Message, String> {
                    let mut x = UdpX::connect(SocketAddr::new(*s, 53)).map_err(|e| e.to_string())?;
                    let q = dns::encode_query(fastrand::u16(..), &QuerySpec::new(name, qtype)).map_err(|e| e.to_string())?;
                    let r = x.exchange(&q, Duration::from_millis(1500)).map_err(|e| e.to_string())?;
                    let m = dns::parse(&r.bytes).map_err(|e| e.to_string())?;
                    // A truncated UDP answer is asked again over TCP.
                    if m.tc() {
                        let ep = Endpoint { transport: Transport::Tcp, addr: SocketAddr::new(*s, 53), host: None, url: None };
                        let mut t = net::open(&ep, Duration::from_millis(1500)).map_err(|e| e.to_string())?;
                        let r = t.exchange(&q, Duration::from_millis(1500)).map_err(|e| e.to_string())?;
                        return dns::parse(&r.bytes).map_err(|e| e.to_string());
                    }
                    Ok(m)
                };
                match attempt() {
                    Ok(m) => return Ok(m),
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }
}
