//! The `OSS_EDGE_*` variables: the HTTP/2 front's listeners and limits (ADR 0058).
//!
//! Every listener is empty, which opens no port, unless its variable names an address.

use std::net::SocketAddr;
use std::path::PathBuf;

use super::write_limit::Bounded;

/// What the operator asked of the protocol front. Read whether or not the node is built with
/// the `edge` feature, so a build without it can refuse the settings instead of ignoring them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeSettings {
    /// `OSS_EDGE_H2C_ADDR`: HTTP/2 over clear text.
    pub h2c: Option<SocketAddr>,
    /// `OSS_EDGE_TLS_ADDR`: TLS with ALPN `h2` and `http/1.1`.
    pub tls: Option<SocketAddr>,
    /// `OSS_EDGE_TLS_CERT`: the PEM certificate chain.
    pub cert: Option<PathBuf>,
    /// `OSS_EDGE_TLS_KEY`: the PEM private key.
    pub key: Option<PathBuf>,
    /// `OSS_EDGE_MAX_STREAMS`; unset is a quarter of the HTTP workers.
    pub max_streams: Option<usize>,
    /// `OSS_EDGE_MAX_CONNECTIONS`.
    pub max_connections: usize,
    /// `OSS_EDGE_HANDSHAKE_TIMEOUT_MS`.
    pub handshake_millis: u64,
    /// `OSS_EDGE_IDLE_TIMEOUT_MS`.
    pub idle_millis: u64,
    /// `OSS_EDGE_STREAM_BUFFER`, in bytes.
    pub stream_buffer: usize,
    /// `OSS_EDGE_CONNECTION_OVERHEAD`, in bytes.
    pub connection_overhead: usize,
    /// `OSS_EDGE_THREADS`.
    pub threads: usize,
}

impl EdgeSettings {
    /// Whether any listener is set.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.h2c.is_some() || self.tls.is_some()
    }
}

pub(in crate::boot) fn read(
    get: &dyn Fn(&str) -> Option<String>,
    bounded: Bounded<'_>,
) -> Result<EdgeSettings, String> {
    let addr = |key: &str| -> Result<Option<SocketAddr>, String> {
        get(key)
            .filter(|v| !v.is_empty())
            .map(|v| v.parse().map_err(|e| format!("{key}: {e}")))
            .transpose()
    };
    let path = |key: &str| get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let count = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    let max_streams = get("OSS_EDGE_MAX_STREAMS")
        .filter(|v| !v.is_empty())
        .map(|_| bounded("OSS_EDGE_MAX_STREAMS", "1", 1, 100_000))
        .transpose()?
        .map(count);
    Ok(EdgeSettings {
        h2c: addr("OSS_EDGE_H2C_ADDR")?,
        tls: addr("OSS_EDGE_TLS_ADDR")?,
        cert: path("OSS_EDGE_TLS_CERT"),
        key: path("OSS_EDGE_TLS_KEY"),
        max_streams,
        max_connections: count(bounded("OSS_EDGE_MAX_CONNECTIONS", "256", 1, 1_000_000)?),
        handshake_millis: bounded("OSS_EDGE_HANDSHAKE_TIMEOUT_MS", "10000", 1, 600_000)?,
        idle_millis: bounded("OSS_EDGE_IDLE_TIMEOUT_MS", "60000", 1, 86_400_000)?,
        stream_buffer: count(bounded(
            "OSS_EDGE_STREAM_BUFFER",
            "65536",
            16_384,
            16 << 20,
        )?),
        connection_overhead: count(bounded(
            "OSS_EDGE_CONNECTION_OVERHEAD",
            "131072",
            0,
            64 << 20,
        )?),
        threads: count(bounded("OSS_EDGE_THREADS", "2", 1, 256)?),
    })
}
