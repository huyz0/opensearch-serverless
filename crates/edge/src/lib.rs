//! The protocol front: HTTP/2 over clear text and over TLS, bridged stream by stream to the
//! node's own HTTP/1.1 server (docs/adr/0058-modern-protocols-front.md).
//!
//! Nothing here knows a handler. A request stream becomes one HTTP/1.1 request on a loopback
//! connection, so the server's limits, error shapes and admission apply unchanged. The front is
//! bounded by construction: `max_streams` in flight and `max_connections` open, charged to the
//! request-body pool when it starts.

mod config;
mod grpc;
mod h3stream;
mod quic;
mod serve;
mod stream;
mod tls;
mod translate;
mod upstream;

use std::io;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;

use http::HeaderValue;
use oss_mem::{Exhausted, MemoryBudget, Pool, Reservation};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{Semaphore, watch};

pub use config::{
    DEFAULT_GRPC_MESSAGE, DEFAULT_H3_OVERHEAD, EdgeConfig, H3_ENDPOINT_BYTES, MIN_STREAM_BUFFER,
    STREAMS_PER_CONNECTION,
};
/// The query types the gRPC front maps, for the node's own tests.
#[doc(hidden)]
pub use grpc::query::COVERED_QUERIES;
/// The OpenSearch protobuf message types, for the node's own tests of the gRPC front.
#[doc(hidden)]
pub use grpc::schema::message as proto_message;

/// Why the front did not start.
#[derive(Debug)]
pub enum EdgeError {
    /// The settings are inconsistent.
    Config(String),
    /// The request-body pool cannot hold the front.
    Memory(Exhausted),
    /// The certificate or key cannot be used.
    Tls(String),
    /// A socket or the runtime could not be set up.
    Io(io::Error),
}

impl From<io::Error> for EdgeError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A running front. Dropping it stops it.
pub struct Edge {
    runtime: Option<Runtime>,
    stop: watch::Sender<bool>,
    h2c: Option<SocketAddr>,
    tls: Option<SocketAddr>,
    h3: Option<SocketAddr>,
    retries: quic::Retries,
    grpc: Option<SocketAddr>,
    _reservation: Reservation,
}

impl Edge {
    /// Binds the configured listeners and starts serving; the front's memory is charged to `budget`.
    ///
    /// # Errors
    ///
    /// [`EdgeError`] when the settings are inconsistent, the pool is too small, the certificate
    /// or key is unusable, or a socket cannot be bound.
    pub fn start(config: EdgeConfig, budget: &MemoryBudget) -> Result<Self, EdgeError> {
        config.validate().map_err(EdgeError::Config)?;
        let acceptor = match (&config.cert, &config.key) {
            (Some(cert), Some(key)) if config.tls.is_some() || config.grpc.is_some() => {
                Some(tls::acceptor(cert, key).map_err(EdgeError::Tls)?)
            }
            _ => None,
        };
        let quic_crypto = match (&config.cert, &config.key, config.h3) {
            (Some(cert), Some(key), Some(_)) => Some(tls::quic(cert, key).map_err(EdgeError::Tls)?),
            _ => None,
        };
        let reservation = budget
            .try_reserve(Pool::RequestBodies, config.charge())
            .map_err(EdgeError::Memory)?;
        let h2c = config.h2c.map(TcpListener::bind).transpose()?;
        let tls_listener = config.tls.map(TcpListener::bind).transpose()?;
        let grpc_listener = config.grpc.map(TcpListener::bind).transpose()?;
        let h3_socket = config.h3.map(UdpSocket::bind).transpose()?;
        if let Some(s) = &h3_socket {
            s.set_nonblocking(true)?;
        }
        let h3_addr = h3_socket.as_ref().map(UdpSocket::local_addr).transpose()?;
        // The port a client is told to use is the one the socket bound.
        // ALLOC: one short header value at start.
        let alt_svc = h3_addr
            .map(|a| HeaderValue::from_str(&format!("h3=\":{}\"; ma=86400", a.port())))
            .transpose()
            // ALLOC: error text at start, once.
            .map_err(|e| EdgeError::Config(format!("alt-svc value: {e}")))?;
        if grpc_listener.is_some() {
            grpc::schema::warm();
        }
        for l in h2c
            .iter()
            .chain(tls_listener.iter())
            .chain(grpc_listener.iter())
        {
            l.set_nonblocking(true)?;
        }
        let h2c_addr = h2c.as_ref().map(TcpListener::local_addr).transpose()?;
        let tls_addr = tls_listener
            .as_ref()
            .map(TcpListener::local_addr)
            .transpose()?;
        let grpc_addr = grpc_listener
            .as_ref()
            .map(TcpListener::local_addr)
            .transpose()?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(config.threads)
            .thread_name("oss-edge")
            .enable_all()
            .build()?;
        let (stop, watcher) = watch::channel(false);
        let retries = quic::Retries::default();
        let shared = Arc::new(stream::Shared {
            streams: Arc::new(Semaphore::new(config.max_streams)),
            connections: Arc::new(Semaphore::new(config.max_connections)),
            config,
        });
        {
            let _guard = runtime.enter();
            if let Some(l) = h2c {
                let l = tokio::net::TcpListener::from_std(l)?;
                runtime.spawn(serve::accept_loop(
                    l,
                    None,
                    serve::Kind::Http,
                    None,
                    Arc::clone(&shared),
                    watcher.clone(),
                ));
            }
            if let (Some(l), Some(a)) = (tls_listener, acceptor.clone()) {
                let l = tokio::net::TcpListener::from_std(l)?;
                runtime.spawn(serve::accept_loop(
                    l,
                    Some(a),
                    serve::Kind::Http,
                    alt_svc,
                    Arc::clone(&shared),
                    watcher.clone(),
                ));
            }
            if let Some(l) = grpc_listener {
                let l = tokio::net::TcpListener::from_std(l)?;
                runtime.spawn(serve::accept_loop(
                    l,
                    acceptor,
                    serve::Kind::Grpc,
                    None,
                    Arc::clone(&shared),
                    watcher.clone(),
                ));
            }
            if let (Some(socket), Some(crypto)) = (h3_socket, quic_crypto) {
                let endpoint = quic::endpoint(socket, crypto, &shared)?;
                runtime.spawn(quic::accept_loop(
                    endpoint,
                    Arc::clone(&shared),
                    Arc::clone(&retries),
                    watcher.clone(),
                ));
            }
        }
        Ok(Self {
            runtime: Some(runtime),
            stop,
            h2c: h2c_addr,
            tls: tls_addr,
            h3: h3_addr,
            retries,
            grpc: grpc_addr,
            _reservation: reservation,
        })
    }

    /// The bound clear-text HTTP/2 address.
    #[must_use]
    pub fn h2c_addr(&self) -> Option<SocketAddr> {
        self.h2c
    }

    /// The bound TLS address.
    #[must_use]
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls
    }

    /// The bound UDP address of the HTTP/3 listener.
    #[must_use]
    pub fn h3_addr(&self) -> Option<SocketAddr> {
        self.h3
    }

    /// How many `Retry` packets the HTTP/3 listener has sent, for the node's own tests.
    #[doc(hidden)]
    #[must_use]
    pub fn h3_retries(&self) -> u64 {
        self.retries.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The bound gRPC address.
    #[must_use]
    pub fn grpc_addr(&self) -> Option<SocketAddr> {
        self.grpc
    }

    /// Stops accepting, drops open connections and returns the memory.
    pub fn shutdown(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        let _ = self.stop.send(true);
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.stop_now();
    }
}
