//! Starting the HTTP/2 front beside the HTTP/1.1 server (docs/adr/0058).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use oss_mem::MemoryBudget;

use super::{BootError, NodeConfig};
use crate::assemble::Running;

/// The address the front bridges to: the server's own, with an unspecified host read as loopback.
fn upstream_of(bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), bound.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), bound.port())
        }
        _ => bound,
    }
}

/// The running front, if any; stopped before the server it bridges to.
pub(super) struct Front {
    #[cfg(feature = "edge")]
    edge: Option<oss_edge::Edge>,
}

impl Front {
    /// Stops the listeners; streams in flight end with them.
    pub(super) fn stop(&mut self) {
        #[cfg(feature = "edge")]
        drop(self.edge.take());
    }
}

/// With the `edge` feature: starts the front when a listener is configured.
#[cfg(feature = "edge")]
pub(super) fn start(
    config: &NodeConfig,
    running: &Running,
    budget: &MemoryBudget,
) -> Result<Front, BootError> {
    use std::time::Duration;
    let s = &config.edge;
    if !s.enabled() {
        return Ok(Front { edge: None });
    }
    let mut edge =
        oss_edge::EdgeConfig::new(upstream_of(running.addr()), config.rest_config().workers);
    edge.h2c = s.h2c;
    edge.tls = s.tls;
    edge.cert.clone_from(&s.cert);
    edge.key.clone_from(&s.key);
    if let Some(n) = s.max_streams {
        edge.max_streams = n;
    }
    edge.max_connections = s.max_connections;
    edge.handshake_timeout = Duration::from_millis(s.handshake_millis);
    edge.idle_timeout = Duration::from_millis(s.idle_millis);
    edge.stream_buffer = s.stream_buffer;
    edge.connection_overhead = s.connection_overhead;
    edge.threads = s.threads;
    oss_edge::Edge::start(edge, budget)
        .map(|e| Front { edge: Some(e) })
        .map_err(|e| BootError::Config(format!("edge: {e:?}")))
}

/// Without it, a listener the operator asked for is refused, not ignored.
#[cfg(not(feature = "edge"))]
pub(super) fn start(
    config: &NodeConfig,
    _running: &Running,
    _budget: &MemoryBudget,
) -> Result<Front, BootError> {
    let _ = upstream_of;
    if config.edge.enabled() {
        return Err(BootError::Config(
            "OSS_EDGE_H2C_ADDR or OSS_EDGE_TLS_ADDR is set, and this build has no `edge` feature"
                .into(),
        ));
    }
    Ok(Front {})
}
