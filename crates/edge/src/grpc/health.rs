//! `grpc.health.v1.Health/Check`, answered from the node's own health route.

use bytes::Bytes;

use super::call;
use super::status::{INVALID_ARGUMENT, NOT_FOUND, Status};
use crate::stream::Shared;

const SERVING: u8 = 1;
const NOT_SERVING: u8 = 2;

/// Service names `Check` knows: the empty name is the whole node.
const KNOWN: [&str; 3] = [
    "",
    "org.opensearch.protobufs.services.DocumentService",
    "org.opensearch.protobufs.services.SearchService",
];

/// `HealthCheckRequest { string service = 1; }`, decoded by hand: the one field is all there is.
fn service_of(message: &[u8]) -> Result<&str, Status> {
    let bad = || Status::new(INVALID_ARGUMENT, "malformed HealthCheckRequest");
    match message {
        [] => Ok(""),
        [0x0a, len, rest @ ..] if *len < 0x80 && rest.len() == usize::from(*len) => {
            std::str::from_utf8(rest).map_err(|_| bad())
        }
        _ => Err(bad()),
    }
}

/// Answers `SERVING` when `/_cluster/health` answers 200, `NOT_SERVING` otherwise.
pub(crate) async fn check(shared: &Shared, message: &[u8]) -> Result<Bytes, Status> {
    let service = service_of(message)?;
    if !KNOWN.contains(&service) {
        return Err(Status::new(NOT_FOUND, "unknown service"));
    }
    let serving = match call::unary(shared, "GET", "/_cluster/health", b"").await {
        Ok(answer) => answer.status == 200,
        Err(_) => false,
    };
    // HealthCheckResponse { ServingStatus status = 1; }
    Ok(Bytes::copy_from_slice(&[
        0x08,
        if serving { SERVING } else { NOT_SERVING },
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_a_service_or_none() {
        assert_eq!(service_of(b"").unwrap(), "");
        assert_eq!(service_of(b"\x0a\x03abc").unwrap(), "abc");
        assert!(service_of(b"\x0a\x05abc").is_err());
        assert!(service_of(b"\x10\x01").is_err());
    }
}
