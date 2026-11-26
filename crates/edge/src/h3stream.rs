//! One HTTP/3 request stream, bridged to one HTTP/1.1 connection on the node's own listener.

use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes};
use h3::error::Code;
use h3::server::{RequestResolver, RequestStream};
use http::{Method, Response, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

use crate::stream::Shared;
use crate::translate::{self, Head, Refusal};
use crate::upstream::Response as Upstream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Send = RequestStream<h3_quinn::SendStream<Bytes>, Bytes>;
type Recv = RequestStream<h3_quinn::RecvStream, Bytes>;

async fn status_only(send: &mut Send, status: StatusCode) {
    if let Ok(response) = Response::builder().status(status).body(())
        && send.send_response(response).await.is_ok()
    {
        let _ = send.finish().await;
    }
}

/// How many `None` answers in a row `piece` reads past before it takes one for the end.
const MAX_EMPTY_FRAMES: u32 = 64;

/// The next piece of the request body, or `None` at its end.
///
/// `h3` answers `None` for an empty DATA frame as it does for the end of the stream, and goes
/// on from the next frame if asked again. Taking the first `None` for the end would serve a body
/// cut short at an empty frame, so a `None` is read past until `MAX_EMPTY_FRAMES` of them in a
/// row. A real end answers `None` at once and every time, so it costs those few ready polls. A
/// body with more empty frames in a row than that is cut there: `h3` gives no way to tell the two
/// apart, and a declared `content-length` refuses the shortfall.
async fn piece(recv: &mut Recv) -> Result<Option<Bytes>, ()> {
    for _ in 0..MAX_EMPTY_FRAMES {
        match recv.recv_data().await {
            Ok(Some(mut data)) => {
                let n = data.remaining();
                if n == 0 {
                    continue;
                }
                return Ok(Some(data.copy_to_bytes(n)));
            }
            Ok(None) => {}
            Err(_) => return Err(()),
        }
    }
    Ok(None)
}

/// Serves one stream to its end. A stream past `max_streams` is refused with `H3_REQUEST_REJECTED`.
pub(crate) async fn serve(
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    shared: Arc<Shared>,
) {
    let permit = Arc::clone(&shared.streams).try_acquire_owned();
    let Ok(Ok((request, stream))) =
        timeout(shared.config.handshake_timeout, resolver.resolve_request()).await
    else {
        return;
    };
    let (mut send, mut recv) = stream.split();
    let Ok(_permit) = permit else {
        send.stop_stream(Code::H3_REQUEST_REJECTED);
        recv.stop_sending(Code::H3_REQUEST_REJECTED);
        return;
    };
    let (parts, ()) = request.into_parts();
    let Ok(length) = translate::declared_length(&parts.headers) else {
        return status_only(&mut send, StatusCode::BAD_REQUEST).await;
    };
    // One piece is read first, to know whether the request has a body at all.
    let first = if length == Some(0) {
        None
    } else {
        match piece(&mut recv).await {
            Ok(first) => first,
            Err(()) => return,
        }
    };
    let end_of_stream = first.is_none();
    let head = Head {
        method: &parts.method,
        target: parts.uri.path_and_query().map_or("", |p| p.as_str()),
        authority: parts.uri.authority().map(http::uri::Authority::as_str),
        headers: &parts.headers,
        length,
        end_of_stream,
    };
    let bytes = match translate::write_head(
        &head,
        shared.config.max_initial_line,
        shared.config.max_header_bytes,
    ) {
        Ok(b) => b,
        Err(Refusal::Malformed(_)) => {
            return status_only(&mut send, StatusCode::BAD_REQUEST).await;
        }
        Err(Refusal::TooLarge) => {
            return status_only(&mut send, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE).await;
        }
    };
    let connected = timeout(CONNECT_TIMEOUT, TcpStream::connect(shared.config.upstream)).await;
    let Ok(Ok(socket)) = connected else {
        return status_only(&mut send, StatusCode::BAD_GATEWAY).await;
    };
    let _ = socket.set_nodelay(true);
    let (rd, mut wr) = socket.into_split();
    if wr.write_all(&bytes).await.is_err() {
        return status_only(&mut send, StatusCode::BAD_GATEWAY).await;
    }
    let head_request = parts.method == Method::HEAD;
    let mut down = std::pin::pin!(download(
        Upstream::new(rd, shared.config.stream_buffer),
        head_request,
        send
    ));
    if end_of_stream {
        let _ = down.await;
        return;
    }
    let mut up = std::pin::pin!(upload(first, recv, wr, length));
    tokio::select! {
        _ = &mut down => {}
        sent = &mut up => {
            if sent.is_ok() {
                let _ = down.await;
            }
        }
    }
}

/// Copies the request body to the socket; the client's credit returns as each piece is read,
/// and a piece is read only after the socket took the one before.
async fn upload(
    first: Option<Bytes>,
    mut recv: Recv,
    mut wr: OwnedWriteHalf,
    length: Option<u64>,
) -> Result<(), ()> {
    let mut sent: u64 = 0;
    let mut next = first;
    loop {
        let data = match next.take() {
            Some(d) => d,
            None => match piece(&mut recv).await? {
                Some(d) => d,
                None => break,
            },
        };
        let n = data.len();
        sent = sent.checked_add(n as u64).ok_or(())?;
        if length.is_some_and(|l| sent > l) {
            return Err(());
        }
        if length.is_none() {
            // ALLOC: one chunk-size line per DATA frame, a few bytes.
            wr.write_all(format!("{n:x}\r\n").as_bytes())
                .await
                .map_err(|_| ())?;
            wr.write_all(&data).await.map_err(|_| ())?;
            wr.write_all(b"\r\n").await.map_err(|_| ())?;
        } else {
            wr.write_all(&data).await.map_err(|_| ())?;
        }
    }
    if length.is_some_and(|l| sent != l) {
        return Err(());
    }
    if length.is_none() {
        wr.write_all(b"0\r\n\r\n").await.map_err(|_| ())?;
    }
    Ok(())
}

/// Reads the answer and sends it a bounded piece at a time; each `send_data` waits for the
/// client's window.
async fn download(
    mut upstream: Upstream<OwnedReadHalf>,
    head_request: bool,
    mut send: Send,
) -> Result<(), ()> {
    let mut head = match upstream.head(head_request).await {
        Ok(h) => h,
        Err(_) => {
            status_only(&mut send, StatusCode::BAD_GATEWAY).await;
            return Err(());
        }
    };
    let mut response = Response::builder()
        .status(head.status)
        .body(())
        .map_err(|_| ())?;
    *response.headers_mut() = std::mem::take(&mut head.headers);
    let bodyless = head.bodyless();
    send.send_response(response).await.map_err(|_| ())?;
    if bodyless {
        send.finish().await.map_err(|_| ())?;
        return Ok(());
    }
    loop {
        match upstream.next(&mut head).await {
            Ok(Some(piece)) => send.send_data(piece).await.map_err(|_| ())?,
            Ok(None) => {
                send.finish().await.map_err(|_| ())?;
                return Ok(());
            }
            Err(_) => {
                send.stop_stream(Code::H3_INTERNAL_ERROR);
                return Err(());
            }
        }
    }
}
