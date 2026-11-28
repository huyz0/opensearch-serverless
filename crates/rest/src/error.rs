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

use oss_mem::Exhausted;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::{self, Write};

use crate::json_out::{write_byte_size, write_str};
use crate::method::{Method, MethodSet};

/// A metadata value of an exception: a string, a number or `[]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Meta {
    /// A string field.
    Str(String),
    /// An integer field.
    Int(u64),
    /// `[]`, the `script_stack` of a script error.
    EmptyList,
}

/// An OpenSearch exception: its `type`, `reason`, metadata fields written
/// after the reason, and an optional cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    kind: Cow<'static, str>,
    reason: String,
    metadata: Vec<(Cow<'static, str>, Meta)>,
    cause: Option<Box<Exception>>,
    own_root: bool,
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
            own_root: false,
        }
    }

    /// Makes this exception its own `root_cause`, whatever its cause: the
    /// shape of a plugin that wraps the Java exception it caught and reports
    /// its own as the root.
    pub fn with_own_root(mut self) -> Self {
        self.own_root = true;
        self
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
        while let (false, Some(c)) = (e.own_root, &e.cause) {
            e = c;
        }
        e
    }

    /// Writes `type`, `reason`, the metadata and (`with_cause`) `caused_by`,
    /// without braces: the members of an `error` object.
    pub fn write_members(&self, out: &mut dyn Write, with_cause: bool) -> io::Result<()> {
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
                Meta::EmptyList => out.write_all(b"[]")?,
            }
        }
        if let (true, Some(cause)) = (with_cause, &self.cause) {
            out.write_all(b",\"caused_by\":{")?;
            cause.write_members(out, true)?;
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
        Self::breaker(e, "in_flight_requests", label, false)
    }

    /// 429 `circuit_breaking_exception` from the breaker named `breaker`,
    /// as `ChildMemoryCircuitBreaker#circuitBreak` words it. `permanent`
    /// marks a request no retry can admit (durability `PERMANENT`, which
    /// OpenSearch gives a request larger than a breaker's whole limit),
    /// else `TRANSIENT`.
    pub fn breaker(e: &Exhausted, breaker: &str, label: &str, permanent: bool) -> Self {
        let wanted = u64::try_from(e.in_use.saturating_add(e.requested)).unwrap_or(u64::MAX);
        let limit = u64::try_from(e.limit).unwrap_or(u64::MAX);
        // ALLOC: error path; a fixed message plus two sizes and the label.
        let mut reason = Vec::with_capacity(160);
        // Writes into a Vec cannot fail.
        let _ = write!(
            reason,
            "[{breaker}] Data too large, data for [{label}] would be [{wanted}/"
        );
        let _ = write_byte_size(&mut reason, wanted);
        let _ = write!(reason, "], which is larger than the limit of [{limit}/");
        let _ = write_byte_size(&mut reason, limit);
        let _ = write!(reason, "]");
        let reason = String::from_utf8_lossy(&reason).into_owned();
        let ex = Exception::new("circuit_breaking_exception", reason)
            .with_meta("bytes_wanted", Meta::Int(wanted))
            .with_meta("bytes_limit", Meta::Int(limit))
            .with_meta(
                "durability",
                Meta::Str(if permanent { "PERMANENT" } else { "TRANSIENT" }.into()),
            );
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
                e.root().write_members(out, false)?;
                out.write_all(b"}],")?;
                e.write_members(out, true)?;
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
#[path = "error_tests.rs"]
mod tests;
