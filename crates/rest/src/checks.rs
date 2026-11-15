//! The checks a request passes before its handler runs: framing and
//! expectation, content type, the URI as Java would parse it, and the query
//! parameters the route takes.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::body::{Chunk, Framing};
use crate::config::ServerConfig;
use crate::conn::GLOBAL_PARAMS;
use crate::error::RestError;
use crate::head::{Head, Version};
use crate::params;
use crate::query::{QueryParams, decode_component};
use crate::router::{PathParam, Route};

/// What the checks before the handler decided.
pub(crate) struct Plan {
    pub(crate) framing: Framing,
    pub(crate) expect_continue: bool,
}

/// Checks that the path parameters decode, and refuses query parameters the
/// route does not take.
pub(crate) fn check_params(
    params: &mut QueryParams,
    bound: &[PathParam],
    route: &Route,
    path: &str,
    query: &str,
) -> Result<(), RestError> {
    for (_, r) in bound {
        decode_component(&path[r.clone()], false)?;
    }
    params.parse(query, path)?;
    let accepted = |n: &str| route.accepts(n) || GLOBAL_PARAMS.contains(&n);
    if params.names(query).all(|n| accepted(&n)) {
        return Ok(());
    }
    // ALLOC: error path; at most `max_query_params` names.
    let names: Vec<_> = params.names(query).filter(|n| !accepted(n)).collect();
    let invalids: BTreeSet<&str> = names.iter().map(AsRef::as_ref).collect();
    let shown = decode_component(path, false).unwrap_or(Cow::Borrowed(path));
    Err(params::unrecognized(
        &shown,
        &invalids,
        route.candidates().chain(GLOBAL_PARAMS),
    ))
}

/// Whether `java.net.URI` parses `target`: `RestController#handleBadRequest`
/// echoes the path only then, and answers "invalid uri has been requested"
/// otherwise, which keeps markup out of the error.
pub(crate) fn java_parses_uri(target: &str) -> bool {
    let b = target.as_bytes();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let bad = c <= b' ' || c == 0x7f || b"\"<>\\^`{|}[]".contains(&c);
        if bad {
            return false;
        }
        if c == b'%' {
            let hex = |k: usize| b.get(k).is_some_and(u8::is_ascii_hexdigit);
            if !hex(i.saturating_add(1)) || !hex(i.saturating_add(2)) {
                return false;
            }
        }
        i = i.saturating_add(1);
    }
    true
}

/// The body's framing and expectation, refused where Netty refuses them.
pub(crate) fn plan(head: &Head, buf: &[u8], c: &ServerConfig) -> Result<Plan, RestError> {
    if head.headers(buf, "content-type").nth(1).is_some() {
        return Err(
            RestError::illegal_argument("only one Content-Type header should be provided")
                .closing(),
        );
    }
    let te = head.header(buf, "transfer-encoding");
    let mut lengths = head.headers(buf, "content-length");
    let framing = match (te, lengths.next()) {
        (Some(_), Some(_)) => {
            return Err(RestError::illegal_argument(
                "both Transfer-Encoding and Content-Length are set",
            )
            .closing());
        }
        (Some(te), None) => {
            if !te.trim().eq_ignore_ascii_case("chunked") || head.version == Version::Http10 {
                return Err(RestError::illegal_argument("unsupported Transfer-Encoding").closing());
            }
            Framing::Chunked(Chunk::Size)
        }
        (None, Some(first)) => {
            let n = parse_length(first)?;
            if lengths.any(|other| parse_length(other).ok() != Some(n)) {
                return Err(
                    RestError::illegal_argument("conflicting Content-Length values").closing(),
                );
            }
            if n > c.max_content_length {
                return Err(RestError::too_large());
            }
            if n == 0 {
                Framing::Empty
            } else {
                Framing::Length(n)
            }
        }
        (None, None) => Framing::Empty,
    };
    let expect_continue = match head.header(buf, "expect") {
        None => false,
        Some(v) if v.trim().eq_ignore_ascii_case("100-continue") => true,
        Some(_) => return Err(RestError::empty(417)),
    };
    Ok(Plan {
        framing,
        expect_continue: expect_continue && framing != Framing::Empty,
    })
}

fn parse_length(v: &str) -> Result<u64, RestError> {
    let v = v.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RestError::illegal_argument("invalid Content-Length").closing());
    }
    v.parse::<u64>().map_err(|_| RestError::too_large())
}

/// The body's media types this edge parses: JSON, and NDJSON for bulk.
/// OpenSearch also takes SMILE, YAML and CBOR; they are refused here with the
/// message OpenSearch gives a type it does not know.
pub(crate) fn content_type(head: &Head, buf: &[u8]) -> Result<(), RestError> {
    let mut values = head.headers(buf, "content-type").peekable();
    if values.peek().is_none() {
        return Err(RestError::simple(406, "Content-Type header is missing").closing());
    }
    // ALLOC: at most `max_headers` values, bounded by the header limit.
    let all: Vec<&str> = values.collect();
    let known = all.iter().all(|v| {
        let media = v.split(';').next().unwrap_or_default().trim();
        media.eq_ignore_ascii_case("application/json")
            || media.eq_ignore_ascii_case("application/x-ndjson")
    });
    if known {
        return Ok(());
    }
    // ALLOC: error path; the header values, bounded by the header limit.
    let joined = all.join(",");
    Err(RestError::simple(
        406,
        // ALLOC: error path; bounded by the request line or a fixed message.
        format!("Content-Type header [{joined}] is not supported"),
    )
    .closing())
}
