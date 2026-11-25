//! The front's settings and the memory they cost.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// The smallest stream buffer: HTTP/2's default maximum frame size.
pub const MIN_STREAM_BUFFER: usize = 16 * 1024;

/// Concurrent streams one connection may open; the connection window is this many stream windows.
pub const STREAMS_PER_CONNECTION: usize = 16;

/// What the front serves and how much of it.
#[derive(Debug, Clone)]
pub struct EdgeConfig {
    /// HTTP/2 over clear text, prior knowledge; `None` opens no port.
    pub h2c: Option<SocketAddr>,
    /// TCP with TLS and ALPN `h2` then `http/1.1`; `None` opens no port.
    pub tls: Option<SocketAddr>,
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
        self.h2c.is_some() || self.tls.is_some()
    }

    /// The bytes charged to the request-body pool when the front starts:
    /// `max_streams x 2 x stream_buffer + max_connections x connection_overhead`.
    #[must_use]
    pub fn charge(&self) -> usize {
        self.max_streams
            .saturating_mul(2)
            .saturating_mul(self.stream_buffer)
            .saturating_add(
                self.max_connections
                    .saturating_mul(self.connection_overhead),
            )
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
        if self.threads == 0 {
            return Err("edge threads must be at least 1".into());
        }
        if self.tls.is_some() && (self.cert.is_none() || self.key.is_none()) {
            return Err("the edge tls listener needs both a certificate and a key".into());
        }
        Ok(())
    }
}
