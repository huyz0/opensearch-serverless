//! The HTTP/3 listener: a QUIC endpoint with address validation, 0-RTT off, and the connection loop.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use bytes::Bytes;
use quinn::{Endpoint, EndpointConfig, Incoming, ServerConfig, TransportConfig, VarInt};
use tokio::sync::watch;
use tokio::time::timeout;

use crate::config::STREAMS_PER_CONNECTION;
use crate::h3stream;
use crate::stream::Shared;

/// Unidirectional streams a client may open: the HTTP/3 control stream and the two QPACK streams.
const UNI_STREAMS: u32 = 3;

/// How many `Retry` packets the listener has sent, for the node's own tests.
pub(crate) type Retries = Arc<AtomicU64>;

/// Builds the endpoint on `socket`; must run inside the tokio runtime.
pub(crate) fn endpoint(
    socket: UdpSocket,
    crypto: quinn::crypto::rustls::QuicServerConfig,
    shared: &Shared,
) -> std::io::Result<Endpoint> {
    let window = VarInt::from_u64(shared.config.stream_buffer as u64).unwrap_or(VarInt::MAX);
    let connection_window = VarInt::from_u64(
        (shared.config.stream_buffer as u64).saturating_mul(STREAMS_PER_CONNECTION as u64),
    )
    .unwrap_or(VarInt::MAX);
    let mut transport = TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(VarInt::from_u32(
            u32::try_from(STREAMS_PER_CONNECTION).unwrap_or(16),
        ))
        .max_concurrent_uni_streams(VarInt::from_u32(UNI_STREAMS))
        .stream_receive_window(window)
        .receive_window(connection_window)
        .send_window(connection_window.into_inner())
        .datagram_receive_buffer_size(None)
        .max_idle_timeout(shared.config.idle_timeout.try_into().ok());
    let mut server = ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(Arc::new(transport));
    Endpoint::new(
        EndpointConfig::default(),
        Some(server),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
}

/// Accepts connections until `stop` fires. A client whose address is not validated gets a `Retry`
/// and holds nothing; one past `max_connections` is refused before the handshake.
pub(crate) async fn accept_loop(
    endpoint: Endpoint,
    shared: Arc<Shared>,
    retries: Retries,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let incoming = tokio::select! {
            _ = stop.changed() => {
                endpoint.close(VarInt::from_u32(0), b"shutdown");
                return;
            }
            next = endpoint.accept() => match next {
                Some(i) => i,
                None => return,
            },
        };
        if !incoming.remote_address_validated() && incoming.may_retry() {
            retries.fetch_add(1, Ordering::Relaxed);
            let _ = incoming.retry();
            continue;
        }
        let Ok(permit) = Arc::clone(&shared.connections).try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let shared = Arc::clone(&shared);
        let mut stop = stop.clone();
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = stop.changed() => {}
                () = connection(incoming, shared) => {}
            }
        });
    }
}

struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn connection(incoming: Incoming, shared: Arc<Shared>) {
    let Ok(connecting) = incoming.accept() else {
        return;
    };
    let Ok(Ok(conn)) = timeout(shared.config.handshake_timeout, connecting).await else {
        return;
    };
    let mut builder = h3::server::builder();
    builder
        .max_field_section_size(shared.config.max_header_bytes as u64)
        .send_grease(false);
    let Ok(Ok(mut h3)) = timeout(
        shared.config.handshake_timeout,
        builder.build::<_, Bytes>(h3_quinn::Connection::new(conn)),
    )
    .await
    else {
        return;
    };
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        // The transport's idle timeout ends a silent connection; this ends one that is
        // open but sends no request.
        let accepted = match timeout(shared.config.idle_timeout, h3.accept()).await {
            Ok(next) => next,
            Err(_) if active.load(Ordering::Acquire) == 0 => return,
            Err(_) => continue,
        };
        let Ok(Some(resolver)) = accepted else {
            return;
        };
        active.fetch_add(1, Ordering::AcqRel);
        let guard = Active(Arc::clone(&active));
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let _guard = guard;
            h3stream::serve(resolver, shared).await;
        });
    }
}
