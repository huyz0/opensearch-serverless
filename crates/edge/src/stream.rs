//! One HTTP/2 request stream, bridged to one HTTP/1.1 connection on the node's own listener.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h2::server::SendResponse;
use h2::{Reason, RecvStream, SendStream};
use http::{Method, Request, Response, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Semaphore;

use crate::config::EdgeConfig;
use crate::translate::{self, Head, Refusal};
use crate::upstream::Response as Upstream;

/// What every stream needs: the settings and the cap on streams in flight.
pub(crate) struct Shared {
    pub(crate) config: EdgeConfig,
    pub(crate) streams: Arc<Semaphore>,
    pub(crate) connections: Arc<Semaphore>,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn status_only(respond: &mut SendResponse<Bytes>, status: StatusCode) {
    if let Ok(response) = Response::builder().status(status).body(()) {
        let _ = respond.send_response(response, true);
    }
}

/// Serves one stream to its end. A stream past `max_streams` is refused with `REFUSED_STREAM`.
pub(crate) async fn serve(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    shared: Arc<Shared>,
) {
    let Ok(_permit) = Arc::clone(&shared.streams).try_acquire_owned() else {
        respond.send_reset(Reason::REFUSED_STREAM);
        return;
    };
    let (parts, body) = request.into_parts();
    let length = match translate::declared_length(&parts.headers) {
        Ok(l) => l,
        Err(_) => return status_only(&mut respond, StatusCode::BAD_REQUEST),
    };
    let end_of_stream = body.is_end_stream();
    let target = parts.uri.path_and_query().map_or("", |p| p.as_str());
    let head = Head {
        method: &parts.method,
        target,
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
        Err(Refusal::Malformed(_)) => return status_only(&mut respond, StatusCode::BAD_REQUEST),
        Err(Refusal::TooLarge) => {
            return status_only(&mut respond, StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
        }
    };
    let connected =
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(shared.config.upstream)).await;
    let Ok(Ok(socket)) = connected else {
        return status_only(&mut respond, StatusCode::BAD_GATEWAY);
    };
    let _ = socket.set_nodelay(true);
    let (rd, mut wr) = socket.into_split();
    if wr.write_all(&bytes).await.is_err() {
        return status_only(&mut respond, StatusCode::BAD_GATEWAY);
    }
    let head_request = parts.method == Method::HEAD;
    let mut down = std::pin::pin!(download(
        Upstream::new(rd, shared.config.stream_buffer),
        head_request,
        respond
    ));
    if end_of_stream {
        let _ = down.await;
        return;
    }
    let mut up = std::pin::pin!(upload(body, wr, length));
    tokio::select! {
        _ = &mut down => {}
        sent = &mut up => {
            if sent.is_ok() {
                let _ = down.await;
            }
        }
    }
}

/// Copies the request body to the socket, returning credit to the client only as the socket takes the bytes.
async fn upload(
    mut body: RecvStream,
    mut wr: OwnedWriteHalf,
    length: Option<u64>,
) -> Result<(), ()> {
    let mut sent: u64 = 0;
    while let Some(next) = body.data().await {
        let data = next.map_err(|_| ())?;
        let n = data.len();
        sent = sent.checked_add(n as u64).ok_or(())?;
        if length.is_some_and(|l| sent > l) {
            return Err(());
        }
        if length.is_none() && n > 0 {
            // ALLOC: one chunk-size line per DATA frame, a few bytes.
            wr.write_all(format!("{n:x}\r\n").as_bytes())
                .await
                .map_err(|_| ())?;
            wr.write_all(&data).await.map_err(|_| ())?;
            wr.write_all(b"\r\n").await.map_err(|_| ())?;
        } else {
            wr.write_all(&data).await.map_err(|_| ())?;
        }
        body.flow_control().release_capacity(n).map_err(|_| ())?;
    }
    if length.is_some_and(|l| sent != l) {
        return Err(());
    }
    if length.is_none() {
        wr.write_all(b"0\r\n\r\n").await.map_err(|_| ())?;
    }
    Ok(())
}

/// Reads the answer and sends it, a bounded piece at a time, within the client's window.
async fn download(
    mut upstream: Upstream<tokio::net::tcp::OwnedReadHalf>,
    head_request: bool,
    mut respond: SendResponse<Bytes>,
) -> Result<(), ()> {
    let mut head = match upstream.head(head_request).await {
        Ok(h) => h,
        Err(_) => {
            status_only(&mut respond, StatusCode::BAD_GATEWAY);
            return Err(());
        }
    };
    let mut response = Response::builder()
        .status(head.status)
        .body(())
        .map_err(|_| ())?;
    *response.headers_mut() = std::mem::take(&mut head.headers);
    let bodyless = head.bodyless();
    let mut send = respond.send_response(response, bodyless).map_err(|_| ())?;
    if bodyless {
        return Ok(());
    }
    loop {
        match upstream.next(&mut head).await {
            Ok(Some(piece)) => send_all(&mut send, piece).await?,
            Ok(None) => {
                send.send_data(Bytes::new(), true).map_err(|_| ())?;
                return Ok(());
            }
            Err(_) => {
                send.send_reset(Reason::INTERNAL_ERROR);
                return Err(());
            }
        }
    }
}

async fn send_all(send: &mut SendStream<Bytes>, mut data: Bytes) -> Result<(), ()> {
    while !data.is_empty() {
        send.reserve_capacity(data.len());
        let granted = std::future::poll_fn(|cx| send.poll_capacity(cx))
            .await
            .ok_or(())?
            .map_err(|_| ())?;
        if granted == 0 {
            continue;
        }
        let part = data.split_to(granted.min(data.len()));
        send.send_data(part, false).map_err(|_| ())?;
    }
    Ok(())
}
