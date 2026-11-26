//! One length-prefixed gRPC message read off a request stream, bounded before it is buffered.

use bytes::{Bytes, BytesMut};
use h2::RecvStream;

use super::status::{INTERNAL, INVALID_ARGUMENT, RESOURCE_EXHAUSTED, Status, UNIMPLEMENTED};

/// The five bytes before each message: a compression flag and a big-endian length.
const PREFIX: usize = 5;

/// Reads exactly one message of at most `max` bytes and the end of the stream.
///
/// A message whose declared length is over `max` is refused from its prefix, before any of it is
/// buffered. Flow-control credit returns as bytes are taken, so the client is paced.
pub(crate) async fn read_one(body: &mut RecvStream, max: usize) -> Result<Bytes, Status> {
    let mut prefix = [0u8; PREFIX];
    let mut have = 0usize;
    let mut message: Option<BytesMut> = None;
    let mut expected = 0usize;
    while let Some(next) = body.data().await {
        // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
        let mut data = next.map_err(|e| Status::new(INTERNAL, format!("request stream: {e}")))?;
        let n = data.len();
        while !data.is_empty() {
            if have < PREFIX {
                let take = PREFIX.saturating_sub(have).min(data.len());
                let end = have.saturating_add(take);
                prefix[have..end].copy_from_slice(&data.split_to(take));
                have = end;
                if have == PREFIX {
                    if prefix[0] != 0 {
                        return Err(Status::new(
                            UNIMPLEMENTED,
                            "compressed messages are not supported",
                        ));
                    }
                    expected = usize::try_from(u32::from_be_bytes([
                        prefix[1], prefix[2], prefix[3], prefix[4],
                    ]))
                    .unwrap_or(usize::MAX);
                    if expected > max {
                        return Err(Status::new(
                            RESOURCE_EXHAUSTED,
                            // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
                            format!("message of {expected} bytes is over the {max} byte limit"),
                        ));
                    }
                    // ALLOC: one message, at most `max` bytes, charged to the request-body pool at start.
                    message = Some(BytesMut::with_capacity(expected));
                }
            } else {
                let buf = message
                    .as_mut()
                    .ok_or_else(|| Status::new(INTERNAL, "no message buffer"))?;
                if buf.len() == expected {
                    return Err(Status::new(
                        INVALID_ARGUMENT,
                        "a unary call takes one message",
                    ));
                }
                let take = expected.saturating_sub(buf.len()).min(data.len());
                buf.extend_from_slice(&data.split_to(take));
            }
        }
        body.flow_control()
            .release_capacity(n)
            // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
            .map_err(|e| Status::new(INTERNAL, format!("flow control: {e}")))?;
    }
    match message {
        Some(buf) if buf.len() == expected => Ok(buf.freeze()),
        _ => Err(Status::new(
            INVALID_ARGUMENT,
            "the request ended inside a message",
        )),
    }
}

/// The wire form of one message: the prefix and the bytes.
pub(crate) fn frame(message: &[u8]) -> Bytes {
    // ALLOC: one response message, bounded by the loopback answer limit in `call`.
    let mut out = BytesMut::with_capacity(PREFIX.saturating_add(message.len()));
    out.extend_from_slice(&[0]);
    out.extend_from_slice(
        &u32::try_from(message.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(message);
    out.freeze()
}
