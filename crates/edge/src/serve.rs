//! Accepting connections: the bounds, the handshakes, and the HTTP/2 connection loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

use crate::config::STREAMS_PER_CONNECTION;
use crate::grpc;
use crate::stream::{self, Shared};

/// What a listener speaks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// HTTP/2 bridged stream by stream.
    Http,
    /// gRPC calls.
    Grpc,
}

/// Accepts on `listener` until `stop` fires; a connection past `max_connections` is closed unread.
pub(crate) async fn accept_loop(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    kind: Kind,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(permit) = Arc::clone(&shared.connections).try_acquire_owned() else {
                    drop(socket);
                    continue;
                };
                let tls = tls.clone();
                let shared = Arc::clone(&shared);
                let mut stop = stop.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        _ = stop.changed() => {}
                        () = connection(socket, tls, kind, shared) => {}
                    }
                });
            }
        }
    }
}

async fn connection(socket: TcpStream, tls: Option<TlsAcceptor>, kind: Kind, shared: Arc<Shared>) {
    let _ = socket.set_nodelay(true);
    let handshake = shared.config.handshake_timeout;
    let Some(acceptor) = tls else {
        h2_connection(socket, kind, shared).await;
        return;
    };
    let Ok(Ok(secured)) = timeout(handshake, acceptor.accept(socket)).await else {
        return;
    };
    if secured.get_ref().1.alpn_protocol() == Some(b"h2".as_slice()) {
        h2_connection(secured, kind, shared).await;
    } else if kind == Kind::Http {
        passthrough(secured, shared).await;
    }
}

/// A TLS client that did not offer `h2`: its HTTP/1.1 bytes go to the server as they are.
async fn passthrough<IO: AsyncRead + AsyncWrite + Unpin>(mut client: IO, shared: Arc<Shared>) {
    let Ok(_stream) = Arc::clone(&shared.streams).try_acquire_owned() else {
        return;
    };
    let Ok(Ok(mut upstream)) = timeout(
        Duration::from_secs(5),
        TcpStream::connect(shared.config.upstream),
    )
    .await
    else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn h2_connection<IO: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: IO,
    kind: Kind,
    shared: Arc<Shared>,
) {
    let window = u32::try_from(shared.config.stream_buffer).unwrap_or(u32::MAX);
    let streams = u32::try_from(STREAMS_PER_CONNECTION).unwrap_or(16);
    let header_list = u32::try_from(shared.config.max_header_bytes).unwrap_or(u32::MAX);
    let mut builder = h2::server::Builder::new();
    builder
        .initial_window_size(window)
        .initial_connection_window_size(window.saturating_mul(streams))
        .max_concurrent_streams(streams)
        .max_header_list_size(header_list)
        .max_send_buffer_size(shared.config.stream_buffer)
        .max_pending_accept_reset_streams(8);
    let Ok(Ok(mut conn)) = timeout(shared.config.handshake_timeout, builder.handshake(io)).await
    else {
        return;
    };
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let accepted = match timeout(shared.config.idle_timeout, conn.accept()).await {
            Ok(next) => next,
            Err(_) if active.load(Ordering::Acquire) == 0 => return,
            Err(_) => continue,
        };
        let Some(Ok((request, respond))) = accepted else {
            return;
        };
        active.fetch_add(1, Ordering::AcqRel);
        let guard = Active(Arc::clone(&active));
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let _guard = guard;
            match kind {
                Kind::Http => stream::serve(request, respond, shared).await,
                Kind::Grpc => grpc::serve(request, respond, shared).await,
            }
        });
    }
}
