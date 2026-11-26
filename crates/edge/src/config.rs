//! The front's settings and the memory they cost.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// The smallest stream buffer: HTTP/2's default maximum frame size.
pub const MIN_STREAM_BUFFER: usize = 16 * 1024;

/// Concurrent streams one connection may open; the connection window is this many stream windows.
pub const STREAMS_PER_CONNECTION: usize = 16;

/// The default longest gRPC message: 4 MiB.
pub const DEFAULT_GRPC_MESSAGE: usize = 4 << 20;

/// How many times `grpc_max_message` a call holds at once: the message, its decoded form, the REST
/// body made from it, and the answer.
const GRPC_CALL_FACTOR: usize = 4;

/// What the front serves and how much of it.
#[derive(Debug, Clone)]
pub struct EdgeConfig {
    /// HTTP/2 over clear text, prior knowledge; `None` opens no port.
    pub h2c: Option<SocketAddr>,
    /// TCP with TLS and ALPN `h2` then `http/1.1`; `None` opens no port.
    pub tls: Option<SocketAddr>,
    /// gRPC over HTTP/2; TLS when a certificate is configured, clear text otherwise; `None` opens no port.
    pub grpc: Option<SocketAddr>,
    /// The longest gRPC message in either direction, in bytes.
    pub grpc_max_message: usize,
    /// PEM certificate chain, required with `tls`.
    pub cert: Option<PathBuf>,
    /// PEM private key, required with `tls`.
    pub key: Option<PathBuf>,
    /// The node's own HTTP/1.1 listener every stream is bridged to.
    pub upstream: SocketAddr,
    /// The HTTP/1.1 server's worker count; `max_streams` is held under a quarter of it.
    pub workers: usize,
    /// Streams in flight across all listeners.
    pub max_streams: usize,
    /// Open edge connections across all listeners.
    pub max_connections: usize,
    /// Time allowed for a TLS and HTTP/2 handshake.
    pub handshake_timeout: Duration,
    /// An edge connection with no stream open is closed after this long.
    pub idle_timeout: Duration,
    /// Bytes of each direction of a stream, and the HTTP/2 stream window.
    pub stream_buffer: usize,
    /// Bytes charged for an open connection's own state.
    pub connection_overhead: usize,
    /// Runtime threads.
    pub threads: usize,
    /// The longest request line the HTTP/1.1 server accepts.
    pub max_initial_line: usize,
    /// The most header bytes the HTTP/1.1 server accepts.
    pub max_header_bytes: usize,
}

impl EdgeConfig {
    /// Defaults for a node whose HTTP/1.1 server has `workers` workers and listens on `upstream`.
    #[must_use]
    pub fn new(upstream: SocketAddr, workers: usize) -> Self {
        Self {
            h2c: None,
            tls: None,
            grpc: None,
            grpc_max_message: DEFAULT_GRPC_MESSAGE,
            cert: None,
            key: None,
            upstream,
            workers,
            max_streams: (workers / 4).max(1),
            max_connections: 256,
            handshake_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(60),
            stream_buffer: 64 * 1024,
            connection_overhead: 128 * 1024,
            threads: 2,
            max_initial_line: 4096,
            max_header_bytes: 16 * 1024,
        }
    }

    /// Whether any listener is configured.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.h2c.is_some() || self.tls.is_some() || self.grpc.is_some()
    }

    /// The bytes charged to the request-body pool when the front starts:
    /// `max_streams x 2 x stream_buffer + max_connections x connection_overhead`, and with a gRPC
    /// listener `max_streams x grpc_max_message x 4` for what a call holds at once: the message, its decoded
    /// form, the REST body made from it, and the answer.
    #[must_use]
    pub fn charge(&self) -> usize {
        let messages = if self.grpc.is_some() {
            self.max_streams
                .saturating_mul(self.grpc_max_message)
                .saturating_mul(GRPC_CALL_FACTOR)
        } else {
            0
        };
        self.max_streams
            .saturating_mul(2)
            .saturating_mul(self.stream_buffer)
            .saturating_add(
                self.max_connections
                    .saturating_mul(self.connection_overhead),
            )
            .saturating_add(messages)
    }

    /// Checks the settings against each other.
    ///
    /// # Errors
    ///
    /// A message naming the setting that is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_streams == 0 {
            return Err("edge max_streams must be at least 1".into());
        }
        if self.max_streams > (self.workers / 4).max(1) {
            // ALLOC: error text at start, once.
            return Err(format!(
                "edge max_streams {} is above a quarter of the {} HTTP workers",
                self.max_streams, self.workers
            ));
        }
        if self.max_connections == 0 {
            return Err("edge max_connections must be at least 1".into());
        }
        if self.stream_buffer < MIN_STREAM_BUFFER || u32::try_from(self.stream_buffer).is_err() {
            // ALLOC: error text at start, once.
            return Err(format!(
                "edge stream_buffer must be at least {MIN_STREAM_BUFFER} bytes and fit 32 bits"
            ));
        }
        if self.grpc.is_some() && self.grpc_max_message == 0 {
            return Err("edge grpc_max_message must be at least 1".into());
        }
        if self.threads == 0 {
            return Err("edge threads must be at least 1".into());
        }
        if self.tls.is_some() && (self.cert.is_none() || self.key.is_none()) {
            return Err("the edge tls listener needs both a certificate and a key".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> EdgeConfig {
        EdgeConfig::new("127.0.0.1:9200".parse().expect("addr"), 16)
    }

    #[test]
    fn a_listener_alone_enables_the_front() {
        let addr: SocketAddr = "127.0.0.1:1".parse().expect("addr");
        assert!(!base().enabled());
        let mut h2c = base();
        h2c.h2c = Some(addr);
        let mut tls = base();
        tls.tls = Some(addr);
        let mut grpc = base();
        grpc.grpc = Some(addr);
        assert!(h2c.enabled() && tls.enabled() && grpc.enabled());
    }

    #[test]
    fn the_default_message_is_four_mebibytes_and_charged_only_with_a_grpc_listener() {
        let mut c = base();
        assert_eq!(c.grpc_max_message, 4 << 20);
        assert_eq!(DEFAULT_GRPC_MESSAGE, 4 << 20);
        let without = c.charge();
        c.grpc = Some("127.0.0.1:1".parse().expect("addr"));
        assert_eq!(c.charge(), without + c.max_streams * 4 * (4 << 20));
    }

    #[test]
    fn the_stream_buffer_floor_is_the_http2_frame_size_and_is_inclusive() {
        assert_eq!(MIN_STREAM_BUFFER, 16_384);
        let mut c = base();
        c.stream_buffer = MIN_STREAM_BUFFER;
        assert!(c.validate().is_ok());
        c.stream_buffer = MIN_STREAM_BUFFER - 1;
        assert!(c.validate().is_err());
    }

    #[test]
    fn a_tls_listener_needs_both_a_certificate_and_a_key() {
        let mut c = base();
        c.tls = Some("127.0.0.1:1".parse().expect("addr"));
        assert!(c.validate().is_err(), "neither");
        c.cert = Some("cert.pem".into());
        assert!(c.validate().is_err(), "no key");
        c.cert = None;
        c.key = Some("key.pem".into());
        assert!(c.validate().is_err(), "no certificate");
        c.cert = Some("cert.pem".into());
        assert!(c.validate().is_ok(), "both");
    }
}
