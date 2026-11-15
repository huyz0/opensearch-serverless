//! Error responses in OpenSearch's shapes.
//!
//! Three shapes exist, and each is ported from where OpenSearch writes it, at
//! huyz0/OpenSearch@49f238ed:
//!
//! - an exception: `{"error":{"root_cause":[..],"type":..,"reason":..},"status":N}`,
//!   from `BytesRestResponse#build` and `OpenSearchException#generateFailureXContent`
//!   (libs/core/src/main/java/org/opensearch/OpenSearchException.java);
//! - a simple error: `{"error":"..","status":N}`, from
//!   `BytesRestResponse#createSimpleErrorResponse` (405), or `{"error":".."}`
//!   without a status, from `RestController#handleBadRequest` (no route);
//! - an empty body, for the 413 Netty's aggregator sends for a body over
//!   `http.max_content_length`.
//!
//! Error bodies are built on the error path only, so their strings are
//! allocated; every one is bounded by the request line or a fixed message.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::{self, Write};

use oss_mem::Exhausted;

use crate::json_out::{write_byte_size, write_str};
use crate::method::{Method, MethodSet};

/// A metadata value of an exception: a string or a number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Meta {
    /// A string field.
    Str(String),
    /// An integer field.
    Int(u64),
}

/// An OpenSearch exception: its `type`, `reason`, metadata fields written
/// after the reason, and an optional cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    kind: Cow<'static, str>,
    reason: String,
    metadata: Vec<(Cow<'static, str>, Meta)>,
    cause: Option<Box<Exception>>,
}

impl Exception {
    /// An exception of `kind` (the underscore-case type, such as
    /// `illegal_argument_exception`) with `reason`.
    pub fn new(kind: impl Into<Cow<'static, str>>, reason: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            reason: reason.into(),
            // ALLOC: error path; no metadata until `with_meta` adds one.
            metadata: Vec::new(),
            cause: None,
        }
    }

    /// Adds a metadata field, written after `reason` in insertion order.
    pub fn with_meta(mut self, key: impl Into<Cow<'static, str>>, value: Meta) -> Self {
        self.metadata.push((key.into(), value));
        self
    }

    /// Sets the cause, written as `caused_by`.
    pub fn caused_by(mut self, cause: Exception) -> Self {
        // ALLOC: error path; one box per cause in the chain.
        self.cause = Some(Box::new(cause));
        self
    }

    /// The exception's type.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The exception's reason.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The innermost cause: `OpenSearchException#guessRootCauses` for a chain
    /// of OpenSearch exceptions.
    fn root(&self) -> &Exception {
        let mut e = self;
        while let Some(c) = &e.cause {
            e = c;
        }
        e
    }

    fn write(&self, out: &mut dyn Write, with_cause: bool) -> io::Result<()> {
        out.write_all(b"\"type\":")?;
        write_str(out, &self.kind)?;
        out.write_all(b",\"reason\":")?;
        write_str(out, &self.reason)?;
        for (key, value) in &self.metadata {
            out.write_all(b",")?;
            write_str(out, key)?;
            out.write_all(b":")?;
            match value {
                Meta::Str(s) => write_str(out, s)?,
                Meta::Int(n) => write!(out, "{n}")?,
            }
        }
        if let (true, Some(cause)) = (with_cause, &self.cause) {
            out.write_all(b",\"caused_by\":{")?;
            cause.write(out, true)?;
            out.write_all(b"}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Body {
    Exception(Exception),
    Simple { message: String, with_status: bool },
    Empty,
}

/// An error answered to the client: a status and one of OpenSearch's error
/// bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestError {
    status: u16,
    body: Body,
    allow: MethodSet,
    close: bool,
}

impl RestError {
    /// `status` with an exception body.
    pub fn exception(status: u16, exception: Exception) -> Self {
        Self::with_body(status, Body::Exception(exception))
    }

    /// 400 `illegal_argument_exception`, what Java's `IllegalArgumentException`
    /// becomes at the REST layer.
    pub fn illegal_argument(reason: impl Into<String>) -> Self {
        Self::exception(400, Exception::new("illegal_argument_exception", reason))
    }

    /// 400 `json_parse_exception` for a malformed JSON body.
    pub fn json_parse(reason: impl Into<String>) -> Self {
        Self::exception(400, Exception::new("json_parse_exception", reason))
    }

    /// 429 `circuit_breaking_exception` for an exhausted memory pool, as the
    /// `in_flight_requests` breaker (`ChildMemoryCircuitBreaker#circuitBreak`)
    /// reports a request it cannot admit: `bytes_wanted` is the pool's use if
    /// the request were admitted.
    pub fn circuit_breaking(e: &Exhausted, label: &str) -> Self {
        let wanted = u64::try_from(e.in_use.saturating_add(e.requested)).unwrap_or(u64::MAX);
        let limit = u64::try_from(e.limit).unwrap_or(u64::MAX);
        // ALLOC: error path; a fixed message plus two sizes and the label.
        let mut reason = Vec::with_capacity(160);
        // Writes into a Vec cannot fail.
        let _ = write!(
            reason,
            "[in_flight_requests] Data too large, data for [{label}] would be [{wanted}/"
        );
        let _ = write_byte_size(&mut reason, wanted);
        let _ = write!(reason, "], which is larger than the limit of [{limit}/");
        let _ = write_byte_size(&mut reason, limit);
        let _ = write!(reason, "]");
        let reason = String::from_utf8_lossy(&reason).into_owned();
        let ex = Exception::new("circuit_breaking_exception", reason)
            .with_meta("bytes_wanted", Meta::Int(wanted))
            .with_meta("bytes_limit", Meta::Int(limit))
            .with_meta("durability", Meta::Str("TRANSIENT".into()));
        Self::exception(Exhausted::HTTP_STATUS, ex)
    }

    /// 413 with no body, and the connection closed: the body was not read.
    pub fn too_large() -> Self {
        Self::empty(413)
    }

    /// `status` with no body, closing the connection: 417 for an
    /// expectation other than `100-continue`, as Netty answers it.
    pub fn empty(status: u16) -> Self {
        let mut e = Self::with_body(status, Body::Empty);
        e.close = true;
        e
    }

    /// 400 `{"error":"no handler found for uri [..] and method [..]"}`, with
    /// no `status` field, as `RestController#handleBadRequest` writes it.
    pub fn no_handler(path: &str, method: Method) -> Self {
        // ALLOC: error path; bounded by the request line.
        let message = format!("no handler found for uri [{path}] and method [{method}]");
        Self::with_body(
            400,
            Body::Simple {
                message,
                with_status: false,
            },
        )
    }

    /// 400 `{"error":"invalid uri has been requested"}`: what
    /// `RestController#handleBadRequest` sends in place of a path that
    /// `java.net.URI` cannot parse.
    pub fn invalid_uri() -> Self {
        Self::with_body(
            400,
            Body::Simple {
                // ALLOC: error path; a fixed message.
                message: "invalid uri has been requested".to_owned(),
                with_status: false,
            },
        )
    }

    /// 405 for a path whose routes take other methods, with the `Allow`
    /// header (`RestController#handleUnsupportedHttpMethod`). `method` is
    /// `None` for a token OpenSearch does not know.
    pub fn method_not_allowed(uri: &str, method: Option<Method>, allowed: MethodSet) -> Self {
        // ALLOC: error path; bounded by the request line.
        let mut message = match method {
            Some(m) => format!("Incorrect HTTP method for uri [{uri}] and method [{m}]"),
            None => "Unexpected HTTP method".to_owned(),
        };
        if !allowed.is_empty() {
            let _ = write!(message, ", allowed: {allowed}");
        }
        let mut e = Self::with_body(
            405,
            Body::Simple {
                message,
                with_status: true,
            },
        );
        e.allow = allowed;
        e
    }

    /// `status` with `{"error":"..","status":N}`.
    pub fn simple(status: u16, message: impl Into<String>) -> Self {
        Self::with_body(
            status,
            Body::Simple {
                message: message.into(),
                with_status: true,
            },
        )
    }

    fn with_body(status: u16, body: Body) -> Self {
        Self {
            status,
            body,
            allow: MethodSet::EMPTY,
            close: false,
        }
    }

    /// Marks the connection to close after this error is sent.
    pub fn closing(mut self) -> Self {
        self.close = true;
        self
    }

    /// The HTTP status.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The exception, when the body is one.
    pub fn as_exception(&self) -> Option<&Exception> {
        match &self.body {
            Body::Exception(e) => Some(e),
            _ => None,
        }
    }

    /// The methods for the `Allow` header; empty for none.
    pub fn allow(&self) -> MethodSet {
        self.allow
    }

    /// Whether the connection closes after this error.
    pub fn closes_connection(&self) -> bool {
        self.close
    }

    /// Whether the body is empty.
    pub fn is_empty(&self) -> bool {
        matches!(self.body, Body::Empty)
    }

    /// Writes the JSON body; nothing for an empty one.
    pub fn write_body(&self, out: &mut dyn Write) -> io::Result<()> {
        match &self.body {
            Body::Empty => Ok(()),
            Body::Simple {
                message,
                with_status,
            } => {
                out.write_all(b"{\"error\":")?;
                write_str(out, message)?;
                if *with_status {
                    write!(out, ",\"status\":{}", self.status)?;
                }
                out.write_all(b"}")
            }
            Body::Exception(e) => {
                out.write_all(b"{\"error\":{\"root_cause\":[{")?;
                e.root().write(out, false)?;
                out.write_all(b"}],")?;
                e.write(out, true)?;
                write!(out, "}},\"status\":{}}}", self.status)
            }
        }
    }
}

impl From<io::Error> for RestError {
    /// An I/O failure while serving: the connection is unusable.
    fn from(e: io::Error) -> Self {
        // ALLOC: error path; bounded by the request line or a fixed message.
        Self::exception(500, Exception::new("io_exception", e.to_string())).closing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_mem::Pool;

    fn body(e: &RestError) -> serde_json::Value {
        let mut out = Vec::new();
        e.write_body(&mut out).expect("vec");
        serde_json::from_slice(&out).expect("valid json")
    }

    #[test]
    fn an_exception_renders_root_cause_type_reason_and_metadata() {
        let e = RestError::exception(
            400,
            Exception::new(
                "resource_already_exists_exception",
                "index [a/u] already exists",
            )
            .with_meta("index", Meta::Str("a".into()))
            .with_meta("index_uuid", Meta::Str("u".into())),
        );
        let want = serde_json::json!({
            "error": {
                "root_cause": [{"type": "resource_already_exists_exception", "reason": "index [a/u] already exists", "index": "a", "index_uuid": "u"}],
                "type": "resource_already_exists_exception", "reason": "index [a/u] already exists", "index": "a", "index_uuid": "u"
            },
            "status": 400
        });
        assert_eq!(body(&e), want);
        assert_eq!(e.status(), 400);
        assert_eq!(
            e.as_exception().map(Exception::kind),
            Some("resource_already_exists_exception")
        );
        assert!(!e.closes_connection() && !e.is_empty());
    }

    #[test]
    fn the_root_cause_is_the_innermost_and_written_without_its_cause() {
        let e = RestError::exception(
            400,
            Exception::new("outer", "o")
                .caused_by(Exception::new("inner", "i").with_meta("n", Meta::Int(3))),
        );
        let want = serde_json::json!({
            "error": {
                "root_cause": [{"type": "inner", "reason": "i", "n": 3}],
                "type": "outer", "reason": "o",
                "caused_by": {"type": "inner", "reason": "i", "n": 3}
            },
            "status": 400
        });
        assert_eq!(body(&e), want);
        assert_eq!(e.as_exception().map(Exception::reason), Some("o"));
    }

    #[test]
    fn circuit_breaking_matches_the_in_flight_breaker_message() {
        let ex = Exhausted {
            pool: Pool::RequestBodies,
            requested: 65_536,
            in_use: 131_072,
            limit: 131_072,
        };
        let e = RestError::circuit_breaking(&ex, "<http_request>");
        assert_eq!(e.status(), 429);
        let want = serde_json::json!({
            "error": {
                "root_cause": [{
                    "type": "circuit_breaking_exception",
                    "reason": "[in_flight_requests] Data too large, data for [<http_request>] would be [196608/192kb], which is larger than the limit of [131072/128kb]",
                    "bytes_wanted": 196_608, "bytes_limit": 131_072, "durability": "TRANSIENT"
                }],
                "type": "circuit_breaking_exception",
                "reason": "[in_flight_requests] Data too large, data for [<http_request>] would be [196608/192kb], which is larger than the limit of [131072/128kb]",
                "bytes_wanted": 196_608, "bytes_limit": 131_072, "durability": "TRANSIENT"
            },
            "status": 429
        });
        assert_eq!(body(&e), want);
    }

    #[test]
    fn simple_errors_with_and_without_status() {
        let e = RestError::no_handler("/nope", Method::Get);
        assert_eq!(
            body(&e),
            serde_json::json!({"error": "no handler found for uri [/nope] and method [GET]"})
        );
        let mut allowed = MethodSet::EMPTY;
        allowed.insert(Method::Get);
        allowed.insert(Method::Post);
        let e = RestError::method_not_allowed("/_search", Some(Method::Delete), allowed);
        assert_eq!(
            body(&e),
            serde_json::json!({"error": "Incorrect HTTP method for uri [/_search] and method [DELETE], allowed: [GET, POST]", "status": 405})
        );
        assert_eq!(e.allow(), allowed);
        let e = RestError::method_not_allowed("/x", None, MethodSet::EMPTY);
        assert_eq!(
            body(&e),
            serde_json::json!({"error": "Unexpected HTTP method", "status": 405})
        );
        let e = RestError::simple(406, "Content-Type header [text/plain] is not supported");
        assert_eq!(e.status(), 406);
        assert!(e.as_exception().is_none());
    }

    #[test]
    fn too_large_is_empty_and_closes() {
        let e = RestError::too_large();
        assert_eq!(e.status(), 413);
        assert!(e.is_empty() && e.closes_connection());
        let mut out = Vec::new();
        e.write_body(&mut out).expect("vec");
        assert!(out.is_empty());
    }

    #[test]
    fn io_errors_close_the_connection() {
        let e = RestError::from(io::Error::other("reset"));
        assert_eq!(e.status(), 500);
        assert!(e.closes_connection());
        assert!(
            RestError::illegal_argument("x")
                .closing()
                .closes_connection()
        );
        assert_eq!(
            RestError::json_parse("bad")
                .as_exception()
                .map(Exception::kind),
            Some("json_parse_exception")
        );
    }
}
