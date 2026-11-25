//! One unary request on the loopback connection to the node's HTTP/1.1 listener.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use super::status::{INTERNAL, RESOURCE_EXHAUSTED, Status, UNAVAILABLE};
use crate::stream::Shared;
use crate::upstream::Response;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The answer: its REST status and body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    #[expect(dead_code, reason = "read by the REST mapping, the next slice")]
    pub(crate) body: Bytes,
}

/// Sends `method target` with `body` (JSON when not empty) and reads the whole answer, which is
/// at most `max_message` bytes: a longer one is `RESOURCE_EXHAUSTED`, never buffered past it.
pub(crate) async fn unary(
    shared: &Shared,
    method: &str,
    target: &str,
    body: &[u8],
) -> Result<Answer, Status> {
    let connected =
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(shared.config.upstream)).await;
    let Ok(Ok(socket)) = connected else {
        return Err(Status::new(
            UNAVAILABLE,
            "the node's listener is not reachable",
        ));
    };
    let _ = socket.set_nodelay(true);
    let (rd, mut wr) = socket.into_split();
    // ALLOC: one request head, a few hundred bytes, plus the body already bounded by `max_message`.
    let mut head = format!(
        "{method} {target} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\naccept: application/json\r\n",
        shared.config.upstream
    );
    if !body.is_empty() || method != "GET" {
        // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
        head.push_str(&format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    let sent = async {
        wr.write_all(head.as_bytes()).await?;
        wr.write_all(body).await
    };
    sent.await
        .map_err(|_| Status::new(UNAVAILABLE, "the node's listener closed the connection"))?;
    let mut upstream = Response::new(rd, shared.config.stream_buffer);
    let mut head = upstream
        .head(false)
        .await
        .map_err(|_| Status::new(INTERNAL, "the node's answer is not valid HTTP"))?;
    let status = head.status.as_u16();
    let limit = shared.config.grpc_max_message;
    let mut out = BytesMut::new();
    while !head.bodyless() {
        match upstream.next(&mut head).await {
            Ok(Some(piece)) => {
                if out.len().saturating_add(piece.len()) > limit {
                    return Err(Status::new(
                        RESOURCE_EXHAUSTED,
                        // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
                        format!("the answer is over the {limit} byte message limit"),
                    ));
                }
                // ALLOC: grows to at most `grpc_max_message`, charged to the request-body pool at start.
                out.extend_from_slice(&piece);
            }
            Ok(None) => break,
            Err(_) => return Err(Status::new(INTERNAL, "the node's answer ended early")),
        }
    }
    Ok(Answer {
        status,
        body: out.freeze(),
    })
}
