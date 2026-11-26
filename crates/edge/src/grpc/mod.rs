//! gRPC over the HTTP/2 front: one unary call is one length-prefixed message in, one out, and a
//! `grpc-status` trailer (docs/specs/grpc-front/SPEC.md).
//!
//! A message is decoded whole, so its size is bounded by `grpc_max_message` and charged to the
//! request-body pool when the front starts. Nothing here knows a handler: a call is mapped to the
//! node's REST request and sent over the loopback connection like any other stream.

mod bulk;
mod call;
mod fields;
mod frame;
mod health;
mod pit;
pub(crate) mod query;
mod respond;
mod rest;
pub(crate) mod schema;
pub(crate) mod search;
mod status;
mod streaming;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h2::Reason;
use h2::server::SendResponse;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};

use self::status::{DEADLINE_EXCEEDED, INVALID_ARGUMENT, Status, UNAVAILABLE, UNIMPLEMENTED};
use crate::stream::{Shared, send_all};

const HEALTH_CHECK: &str = "/grpc.health.v1.Health/Check";
const BULK: &str = "/org.opensearch.protobufs.services.DocumentService/Bulk";
const SEARCH: &str = "/org.opensearch.protobufs.services.SearchService/Search";
const STREAM_SEARCH: &str = "/org.opensearch.protobufs.services.SearchService/ServerStreamSearch";
const CREATE_PIT: &str = "/org.opensearch.protobufs.services.SearchService/CreatePit";
const DELETE_PIT: &str = "/org.opensearch.protobufs.services.SearchService/DeletePit";

/// The deadline in a `grpc-timeout` header: up to eight digits and a unit.
fn deadline(value: &str) -> Option<Duration> {
    let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    match unit {
        "H" => n.checked_mul(3600).map(Duration::from_secs),
        "M" => n.checked_mul(60).map(Duration::from_secs),
        "S" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_millis(n)),
        "u" => Some(Duration::from_micros(n)),
        "n" => Some(Duration::from_nanos(n)),
        _ => None,
    }
}

/// A gRPC answer with no message: headers and status in one frame (the "Trailers-Only" form).
pub(crate) fn trailers_only(respond: &mut SendResponse<Bytes>, status: &Status) {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/grpc")
        // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
        .header("grpc-status", status.code.to_string());
    if !status.message.is_empty()
        && let Ok(v) = HeaderValue::from_str(&status::encode_message(&status.message))
    {
        builder = builder.header("grpc-message", v);
    }
    if let Ok(response) = builder.body(()) {
        let _ = respond.send_response(response, true);
    }
}

fn plain(respond: &mut SendResponse<Bytes>, code: StatusCode) {
    if let Ok(response) = Response::builder().status(code).body(()) {
        let _ = respond.send_response(response, true);
    }
}

/// Serves one gRPC call to its end.
pub(crate) async fn serve(
    request: Request<h2::RecvStream>,
    mut respond: SendResponse<Bytes>,
    shared: Arc<Shared>,
) {
    let Ok(_permit) = Arc::clone(&shared.streams).try_acquire_owned() else {
        trailers_only(
            &mut respond,
            &Status::new(UNAVAILABLE, "too many streams in flight"),
        );
        return;
    };
    let (parts, mut body) = request.into_parts();
    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let grpc_proto = content_type == "application/grpc" || content_type == "application/grpc+proto";
    if parts.method != Method::POST || !grpc_proto {
        return plain(&mut respond, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    if parts
        .headers
        .get("grpc-encoding")
        .is_some_and(|v| v.as_bytes() != b"identity")
    {
        let s = Status::new(UNIMPLEMENTED, "compressed messages are not supported");
        return trailers_only(&mut respond, &s);
    }
    let limit = match parts.headers.get("grpc-timeout") {
        None => None,
        Some(v) => match v.to_str().ok().and_then(deadline) {
            Some(d) => Some(d),
            None => {
                let s = Status::new(INVALID_ARGUMENT, "malformed grpc-timeout");
                return trailers_only(&mut respond, &s);
            }
        },
    };
    // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
    let path = parts.uri.path().to_owned();
    if !matches!(
        path.as_str(),
        HEALTH_CHECK | BULK | SEARCH | STREAM_SEARCH | CREATE_PIT | DELETE_PIT
    ) {
        // ALLOC: a status line or request head, bounded by the head limits (16 KiB).
        let s = Status::new(UNIMPLEMENTED, format!("{path} is not served"));
        return trailers_only(&mut respond, &s);
    }
    let message = match frame::read_one(&mut body, shared.config.grpc_max_message).await {
        Ok(m) => m,
        Err(s) => return trailers_only(&mut respond, &s),
    };
    if path == STREAM_SEARCH {
        return match search::stream_request(&message) {
            Ok(plan) => streaming::serve(&shared, &mut respond, &plan, limit).await,
            Err(s) => trailers_only(&mut respond, &s),
        };
    }
    let work = async {
        let (rest, response) = match path.as_str() {
            BULK => (bulk::request(&message)?, "BulkResponse"),
            SEARCH => (search::request(&message)?, "SearchResponse"),
            CREATE_PIT => (pit::create(&message)?, "CreatePITResponse"),
            DELETE_PIT => (pit::delete(&message)?, "DeletePITResponse"),
            _ => return health::check(&shared, &message).await,
        };
        rest::roundtrip(&shared, &rest, &schema::message(response)).await
    };
    let answer = match limit {
        Some(d) => tokio::time::timeout(d, work)
            .await
            .unwrap_or_else(|_| Err(Status::new(DEADLINE_EXCEEDED, "deadline exceeded"))),
        None => work.await,
    };
    match answer {
        Ok(bytes) => reply(&mut respond, &bytes).await,
        Err(s) => trailers_only(&mut respond, &s),
    }
}

/// Sends one message and the `OK` trailer, within the client's window.
async fn reply(respond: &mut SendResponse<Bytes>, message: &[u8]) {
    let Ok(response) = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/grpc")
        .body(())
    else {
        return;
    };
    let Ok(mut send) = respond.send_response(response, false) else {
        return;
    };
    if send_all(&mut send, frame::frame(message)).await.is_err() {
        send.send_reset(Reason::INTERNAL_ERROR);
        return;
    }
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("0"));
    if send.send_trailers(trailers).is_err() {
        send.send_reset(Reason::INTERNAL_ERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_parse_with_their_units() {
        assert_eq!(deadline("50m"), Some(Duration::from_millis(50)));
        assert_eq!(deadline("2S"), Some(Duration::from_secs(2)));
        assert_eq!(deadline("1H"), Some(Duration::from_secs(3600)));
        assert_eq!(deadline("5M"), Some(Duration::from_secs(300)));
        assert_eq!(deadline("7u"), Some(Duration::from_micros(7)));
        assert_eq!(deadline("9n"), Some(Duration::from_nanos(9)));
        assert_eq!(deadline("12345678S"), Some(Duration::from_secs(12_345_678)));
        assert_eq!(deadline("123456789S"), None);
        assert_eq!(deadline("S"), None);
        assert_eq!(deadline("5x"), None);
        assert_eq!(deadline(""), None);
    }
}
