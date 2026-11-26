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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the REST mapping, the next slice")
    )]
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
    // ALLOC: a content-length line, a few bytes.
    head.push_str(&format!(
        "content-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EdgeConfig;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Semaphore;

    /// A node stand-in that reads one request head and writes `answer`, returning the head.
    async fn stub(answer: Vec<u8>) -> (Shared, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let mut config = EdgeConfig::new(listener.local_addr().expect("addr"), 16);
        config.grpc_max_message = 8;
        let shared = Shared {
            config,
            streams: Arc::new(Semaphore::new(1)),
            connections: Arc::new(Semaphore::new(1)),
        };
        let seen = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            let mut buf = [0u8; 2048];
            let n = socket.read(&mut buf).await.expect("reads");
            socket.write_all(&answer).await.expect("writes");
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (shared, seen)
    }

    fn answer(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn the_request_carries_its_body_and_the_answer_comes_back() {
        let (shared, seen) = stub(answer("200 OK", "12345678")).await;
        let got = unary(&shared, "POST", "/x", b"{}").await.expect("answer");
        assert_eq!((got.status, &got.body[..]), (200, &b"12345678"[..]));
        let head = seen.await.expect("joined");
        assert!(head.starts_with("POST /x HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("content-length: 2\r\n"), "{head}");
    }

    #[tokio::test]
    async fn an_answer_over_the_message_limit_is_refused() {
        let (shared, _seen) = stub(answer("200 OK", "123456789")).await;
        let refused = unary(&shared, "GET", "/x", b"").await;
        assert_eq!(refused.err().map(|s| s.code), Some(RESOURCE_EXHAUSTED));
    }

    #[tokio::test]
    async fn an_answer_without_a_body_ends_at_its_head() {
        let (shared, _seen) =
            stub(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".to_vec()).await;
        let got = unary(&shared, "GET", "/x", b"").await.expect("answer");
        assert_eq!((got.status, got.body.len()), (204, 0));
    }
}
