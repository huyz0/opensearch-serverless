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
    /// `OSS_EDGE_GRPC_ADDR`: gRPC over HTTP/2, over TLS when a certificate is set.
    pub grpc: Option<SocketAddr>,
    /// `OSS_EDGE_GRPC_MAX_MESSAGE`, in bytes; held under `max_content_length`.
    pub grpc_max_message: usize,
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
        self.h2c.is_some() || self.tls.is_some() || self.grpc.is_some()
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
        grpc: addr("OSS_EDGE_GRPC_ADDR")?,
        grpc_max_message: count(bounded("OSS_EDGE_GRPC_MAX_MESSAGE", "1048576", 1, 1 << 30)?),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Result<EdgeSettings, String> {
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_owned())
        };
        let bounded = |key: &str, default: &str, low: u64, high: u64| -> Result<u64, String> {
            let value: u64 = get(key)
                .unwrap_or_else(|| default.to_owned())
                .parse()
                .map_err(|e| format!("{key}: {e}"))?;
            if (low..=high).contains(&value) {
                Ok(value)
            } else {
                Err(format!("{key} [{value}] is not within {low} to {high}"))
            }
        };
        read(&get, &bounded)
    }

    #[test]
    fn nothing_is_listened_on_unless_an_address_is_set() {
        let s = settings(&[]).expect("defaults");
        assert!(!s.enabled());
        assert_eq!(
            (s.h2c, s.tls, s.grpc, s.cert, s.key, s.max_streams),
            (None, None, None, None, None, None)
        );
        assert_eq!(
            (s.max_connections, s.handshake_millis, s.idle_millis),
            (256, 10_000, 60_000)
        );
        assert_eq!(
            (s.stream_buffer, s.connection_overhead, s.threads),
            (65_536, 131_072, 2)
        );
        assert_eq!(s.grpc_max_message, 1 << 20);
        let empty = settings(&[("OSS_EDGE_H2C_ADDR", "")]).expect("empty is unset");
        assert!(!empty.enabled());
    }

    #[test]
    fn each_listener_address_alone_enables_the_front() {
        for key in [
            "OSS_EDGE_H2C_ADDR",
            "OSS_EDGE_TLS_ADDR",
            "OSS_EDGE_GRPC_ADDR",
        ] {
            let s = settings(&[(key, "127.0.0.1:9300")]).expect("parses");
            assert!(s.enabled(), "{key}");
        }
        let s = settings(&[
            ("OSS_EDGE_H2C_ADDR", "127.0.0.1:1"),
            ("OSS_EDGE_TLS_ADDR", "127.0.0.1:2"),
            ("OSS_EDGE_GRPC_ADDR", "127.0.0.1:3"),
            ("OSS_EDGE_TLS_CERT", "/c.pem"),
            ("OSS_EDGE_TLS_KEY", "/k.pem"),
            ("OSS_EDGE_MAX_STREAMS", "3"),
            ("OSS_EDGE_GRPC_MAX_MESSAGE", "1000"),
        ])
        .expect("parses");
        assert_eq!(s.h2c, Some("127.0.0.1:1".parse().expect("addr")));
        assert_eq!(s.tls, Some("127.0.0.1:2".parse().expect("addr")));
        assert_eq!(s.grpc, Some("127.0.0.1:3".parse().expect("addr")));
        assert_eq!(s.cert, Some(PathBuf::from("/c.pem")));
        assert_eq!(s.key, Some(PathBuf::from("/k.pem")));
        assert_eq!((s.max_streams, s.grpc_max_message), (Some(3), 1000));
    }

    #[test]
    fn a_bad_address_or_an_out_of_range_value_is_refused_by_name() {
        let e = settings(&[("OSS_EDGE_GRPC_ADDR", "nope")]).expect_err("bad address");
        assert!(e.contains("OSS_EDGE_GRPC_ADDR"), "{e}");
        let e = settings(&[("OSS_EDGE_MAX_CONNECTIONS", "0")]).expect_err("low");
        assert!(e.contains("OSS_EDGE_MAX_CONNECTIONS"), "{e}");
        let e = settings(&[("OSS_EDGE_STREAM_BUFFER", "100")]).expect_err("small buffer");
        assert!(e.contains("OSS_EDGE_STREAM_BUFFER"), "{e}");
        let e = settings(&[("OSS_EDGE_GRPC_MAX_MESSAGE", "0")]).expect_err("zero message");
        assert!(e.contains("OSS_EDGE_GRPC_MAX_MESSAGE"), "{e}");
    }
}
