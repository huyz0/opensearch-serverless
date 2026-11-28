//! Where a delivery may connect: the address rules and the resolver that
//! enforces them (ADR 0081).
//!
//! A tenant names the URL a webhook posts to, so the node is an HTTP client
//! for its callers. Every address a name resolves to is checked, one bad
//! record refuses the name, and the connection is made to an address that was
//! checked, so a name that answers differently the second time is never
//! asked again. The host name stays the request's, so the certificate check
//! and `Host` are the name's.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use ureq::Error;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{ArrayVec, DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;

/// Resolves a name and port to addresses; replaced in tests.
pub type Lookup = Arc<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The system's resolver.
#[must_use]
pub fn system_lookup() -> Lookup {
    Arc::new(|host, port| Ok((host, port).to_socket_addrs()?.collect()))
}

fn v4_public(a: Ipv4Addr) -> bool {
    let [o0, o1, ..] = a.octets();
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
        || a.is_multicast()
        || a.is_documentation()
        || o0 == 0
        || o0 >= 240
        || (o0 == 100 && (64..128).contains(&o1))
        || (o0 == 198 && (18..20).contains(&o1)))
}

fn v6_public(a: Ipv6Addr) -> bool {
    if let Some(v4) = a.to_ipv4_mapped() {
        return v4_public(v4);
    }
    let [s0, s1, s2, s3, s4, s5, s6, s7] = a.segments();
    let embedded = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        v4_public(Ipv4Addr::new(a, b, c, d))
    };
    // NAT64 (64:ff9b::/96) carries an IPv4 address in its last 32 bits.
    if s0 == 0x64 && s1 == 0xff9b && [s2, s3, s4, s5].iter().all(|x| *x == 0) {
        return embedded(s6, s7);
    }
    // 6to4 (2002::/16) carries one in bits 16 to 47.
    if s0 == 0x2002 {
        return embedded(s1, s2);
    }
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_multicast()
        || (s0 & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (s0 & 0xffc0) == 0xfe80 // link local fe80::/10
        || (s0 & 0xffc0) == 0xfec0 // site local, deprecated
        || (s0 == 0x2001 && s1 == 0x0db8)) // documentation
}

/// Whether a delivery may connect to `ip`: a public unicast address, or any
/// when the operator allows private hosts.
#[must_use]
pub fn permitted(ip: IpAddr, allow_private: bool) -> bool {
    allow_private
        || match ip {
            IpAddr::V4(a) => v4_public(a),
            IpAddr::V6(a) => v6_public(a),
        }
}

/// The operator's rules, read once at start.
#[derive(Clone, Debug)]
pub struct EgressPolicy {
    /// Connect to private, loopback and link-local addresses too.
    pub allow_private: bool,
    /// The ports a URL may name.
    pub ports: Vec<u16>,
    /// Host names that are never connected to, lower case.
    pub deny_hosts: Vec<String>,
}

impl Default for EgressPolicy {
    fn default() -> Self {
        Self {
            allow_private: false,
            ports: vec![80, 443],
            deny_hosts: Vec::new(),
        }
    }
}

impl EgressPolicy {
    /// The policy of the node's environment: `OSS_NOTIFICATIONS_ALLOW_PRIVATE`,
    /// `OSS_NOTIFICATIONS_PORTS` (comma list) and `OSS_NOTIFICATIONS_DENY_HOSTS`
    /// (comma list). These are operator settings, never cluster settings.
    #[must_use]
    pub fn from_env() -> Self {
        let mut p = Self::default();
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        p.allow_private = get("OSS_NOTIFICATIONS_ALLOW_PRIVATE").is_some_and(|v| v == "true");
        if let Some(v) = get("OSS_NOTIFICATIONS_PORTS") {
            let ports: Vec<u16> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if !ports.is_empty() {
                p.ports = ports;
            }
        }
        if let Some(v) = get("OSS_NOTIFICATIONS_DENY_HOSTS") {
            p.deny_hosts = v
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
        }
        p
    }

    /// Why a URL's authority may not be used, before any lookup.
    pub(super) fn refuses(&self, host: &str, port: u16) -> Option<&'static str> {
        if !self.ports.contains(&port) {
            return Some("blocked");
        }
        self.refuses_host(host)
    }

    /// As [`EgressPolicy::refuses`] for the name alone, for a host and port
    /// the operator declared, whose port is not a webhook's to choose.
    pub(super) fn refuses_host(&self, host: &str) -> Option<&'static str> {
        let host = host.trim_end_matches('.').to_lowercase();
        if self.deny_hosts.contains(&host) {
            return Some("blocked");
        }
        // A literal that is not in dotted or colon form (decimal, octal, hex)
        // is read as an address by some stacks and as a name by others.
        let numeric = host.split('.').all(|l| {
            !l.is_empty() && (l.bytes().all(|b| b.is_ascii_digit()) || l.starts_with("0x"))
        });
        if numeric && host.parse::<Ipv4Addr>().is_err() {
            return Some("blocked");
        }
        None
    }
}

/// The resolver a delivery's agent uses: the lookup, then the address rules.
pub(super) struct GuardedResolver {
    pub(super) lookup: Lookup,
    pub(super) policy: EgressPolicy,
}

impl fmt::Debug for GuardedResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GuardedResolver")
    }
}

impl Resolver for GuardedResolver {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, Error> {
        let (Some(scheme), Some(authority)) = (uri.scheme(), uri.authority()) else {
            return Err(Error::HostNotFound);
        };
        let host = authority.host().trim_matches(['[', ']']);
        let port = DefaultResolver::host_and_port(scheme, authority)
            .and_then(|hp| hp.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()))
            .ok_or(Error::HostNotFound)?;
        if self.policy.refuses(host, port).is_some() {
            return Err(Error::HostNotFound);
        }
        let addrs = match host.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => (self.lookup)(host, port).map_err(|_| Error::HostNotFound)?,
        };
        // One address that may not be used refuses the name: a mixed answer
        // is how a rebinding name reaches an internal host.
        if addrs.is_empty()
            || addrs
                .iter()
                .any(|a| !permitted(a.ip(), self.policy.allow_private))
        {
            return Err(Error::HostNotFound);
        }
        let mut out: ResolvedSocketAddrs =
            ArrayVec::from_fn(|_| SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0));
        for a in addrs.into_iter().take(16) {
            out.push(a);
        }
        Ok(out)
    }
}
