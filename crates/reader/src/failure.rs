//! A failed search's error body, in OpenSearch's shape.
//!
//! Ported from `OpenSearchException#generateFailureXContent`,
//! `#guessRootCauses` and `#generateThrowableXContent`
//! (libs/core/src/main/java/org/opensearch/OpenSearchException.java) and
//! `SearchPhaseExecutionException#metadataToXContent`
//! (server/src/main/java/org/opensearch/action/search/SearchPhaseExecutionException.java)
//! at OpenSearch 3.10.0. An error is a chain of causes; each one is an
//! OpenSearch exception or another throwable, which decides where the root
//! cause is found: an OpenSearch exception's root is its cause's when that
//! cause is one too, and otherwise itself.

use std::io::{self, Write};

use oss_rest::write_json_str;

/// A metadata value written after an exception's reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Meta {
    /// A string.
    Str(String),
    /// An integer.
    Int(u64),
}

/// One exception of a chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The underscore-case type, such as `parsing_exception`.
    pub kind: &'static str,
    /// The message.
    pub reason: String,
    /// Fields written after the reason, in order.
    pub meta: Vec<(&'static str, Meta)>,
    /// Whether it is an `OpenSearchException`, whose root cause may lie in
    /// its cause.
    pub opensearch: bool,
    /// The cause, written as `caused_by`.
    pub cause: Option<Box<Failure>>,
}

impl Failure {
    /// An exception that is not an `OpenSearchException`.
    pub fn plain(kind: &'static str, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
            // ALLOC: error path; metadata added one field at a time.
            meta: Vec::new(),
            opensearch: false,
            cause: None,
        }
    }

    /// An `OpenSearchException`.
    pub fn opensearch(kind: &'static str, reason: impl Into<String>) -> Self {
        Self {
            opensearch: true,
            ..Self::plain(kind, reason)
        }
    }

    /// Adds a metadata field.
    pub fn with_meta(mut self, key: &'static str, value: Meta) -> Self {
        self.meta.push((key, value));
        self
    }

    /// Sets the cause.
    pub fn caused_by(mut self, cause: Failure) -> Self {
        // ALLOC: error path; one box per cause in the chain.
        self.cause = Some(Box::new(cause));
        self
    }

    /// `guessRootCauses` for a chain without shard failures: an
    /// OpenSearch exception unwraps to an OpenSearch cause, and an
    /// `XContentParseException` to a cause that is either kind, since the
    /// innermost parse failure is the one that tells the user most.
    pub fn root(&self) -> &Failure {
        let mut f = self;
        while let Some(cause) = f.cause.as_deref() {
            let xcontent = |e: &Failure| e.kind == "x_content_parse_exception";
            let unwraps = if f.opensearch {
                cause.opensearch
            } else {
                xcontent(f) && (cause.opensearch || xcontent(cause))
            };
            if !unwraps {
                break;
            }
            f = cause;
        }
        f
    }

    /// Writes the fields of the exception object (without braces).
    pub fn write_fields(&self, out: &mut dyn Write, with_cause: bool) -> io::Result<()> {
        out.write_all(b"\"type\":")?;
        write_json_str(out, self.kind)?;
        out.write_all(b",\"reason\":")?;
        write_json_str(out, &self.reason)?;
        for (key, value) in &self.meta {
            out.write_all(b",")?;
            write_json_str(out, key)?;
            out.write_all(b":")?;
            match value {
                Meta::Str(s) => write_json_str(out, s)?,
                Meta::Int(n) => write!(out, "{n}")?,
            }
        }
        if let (true, Some(cause)) = (with_cause, &self.cause) {
            out.write_all(b",\"caused_by\":{")?;
            cause.write_fields(out, true)?;
            out.write_all(b"}")?;
        }
        Ok(())
    }
}

/// Where a shard-level failure happened, for `failed_shards`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardFailure {
    /// The index.
    pub index: String,
    /// The shard number.
    pub shard: u32,
    /// The node that ran it.
    pub node: String,
}

/// A search's failure: an HTTP status and the exception to answer with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchError {
    /// The HTTP status.
    pub status: u16,
    /// The exception.
    pub failure: Failure,
    /// Set when the failure happened on a shard, after the request was
    /// parsed: the body is then `SearchPhaseExecutionException`'s.
    pub shard: Option<ShardFailure>,
}

impl SearchError {
    /// A failure of the request itself.
    pub fn request(status: u16, failure: Failure) -> Self {
        Self {
            status,
            failure,
            shard: None,
        }
    }

    /// A failure in the query phase on one shard, which OpenSearch reports
    /// as every shard failing (`all shards failed`).
    pub fn on_shard(status: u16, failure: Failure, shard: ShardFailure) -> Self {
        Self {
            status,
            failure,
            shard: Some(shard),
        }
    }

    /// Writes the JSON error body.
    pub fn write_body(&self, out: &mut dyn Write) -> io::Result<()> {
        out.write_all(b"{\"error\":{\"root_cause\":[{")?;
        self.failure.root().write_fields(out, false)?;
        out.write_all(b"}],")?;
        match &self.shard {
            None => self.failure.write_fields(out, true)?,
            Some(shard) => {
                out.write_all(
                    b"\"type\":\"search_phase_execution_exception\",\"reason\":\"all shards failed\",\
                      \"phase\":\"query\",\"grouped\":true,\"failed_shards\":[{\"shard\":",
                )?;
                write!(out, "{}", shard.shard)?;
                out.write_all(b",\"index\":")?;
                write_json_str(out, &shard.index)?;
                out.write_all(b",\"node\":")?;
                write_json_str(out, &shard.node)?;
                out.write_all(b",\"reason\":{")?;
                self.failure.write_fields(out, true)?;
                // The phase failure's cause is the shard failure's exception.
                // One that is not an OpenSearch exception is wrapped on its
                // way to the coordinating node, and the wrapper takes its
                // type and message, so it is written twice.
                out.write_all(b"}}],\"caused_by\":{")?;
                if self.failure.opensearch {
                    self.failure.write_fields(out, true)?;
                    out.write_all(b"}")?;
                } else {
                    self.failure.write_fields(out, false)?;
                    out.write_all(b",\"caused_by\":{")?;
                    self.failure.write_fields(out, true)?;
                    out.write_all(b"}}")?;
                }
            }
        }
        write!(out, "}},\"status\":{}}}", self.status)
    }

    /// The body as bytes.
    pub fn to_body(&self) -> Vec<u8> {
        // ALLOC: error path; one error body, bounded by its messages.
        let mut out = Vec::new();
        // Writing to a Vec cannot fail.
        let _ = self.write_body(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(e: &SearchError) -> String {
        String::from_utf8(e.to_body()).expect("utf-8")
    }

    #[test]
    fn an_opensearch_exception_with_a_plain_cause_is_its_own_root() {
        let e = SearchError::request(
            400,
            Failure::opensearch("parsing_exception", "unknown query [x]")
                .with_meta("line", Meta::Int(1))
                .with_meta("col", Meta::Int(27))
                .caused_by(Failure::plain(
                    "named_object_not_found_exception",
                    "[1:27] unknown field [x]",
                )),
        );
        assert_eq!(
            body(&e),
            r#"{"error":{"root_cause":[{"type":"parsing_exception","reason":"unknown query [x]","line":1,"col":27}],"type":"parsing_exception","reason":"unknown query [x]","line":1,"col":27,"caused_by":{"type":"named_object_not_found_exception","reason":"[1:27] unknown field [x]"}},"status":400}"#
        );
    }

    #[test]
    fn a_chain_of_opensearch_exceptions_roots_at_the_last() {
        let inner = Failure::opensearch("b", "inner").with_meta("index", Meta::Str("i".into()));
        let f = Failure::opensearch("a", "outer").caused_by(inner.clone());
        assert_eq!(f.root(), &inner);
        let plain = Failure::plain("x", "top").caused_by(Failure::opensearch("y", "under"));
        assert_eq!(plain.root().kind, "x", "a plain exception is its own root");
        let parse = Failure::plain("x_content_parse_exception", "outer").caused_by(
            Failure::plain("x_content_parse_exception", "middle").caused_by(
                Failure::opensearch("parsing_exception", "inner")
                    .caused_by(Failure::plain("named_object_not_found_exception", "x")),
            ),
        );
        assert_eq!(
            parse.root().reason,
            "inner",
            "parse failures unwrap to the innermost"
        );
        let stops = Failure::plain("x_content_parse_exception", "outer")
            .caused_by(Failure::plain("illegal_argument_exception", "x"));
        assert_eq!(stops.root().reason, "outer");
    }

    #[test]
    fn an_opensearch_shard_failure_is_not_written_twice() {
        let e = SearchError::on_shard(
            400,
            Failure::opensearch("query_shard_exception", "failed to create query: x")
                .with_meta("index", Meta::Str("probe".into()))
                .caused_by(Failure::plain("number_format_exception", "x")),
            ShardFailure {
                index: "probe".into(),
                shard: 0,
                node: "n".into(),
            },
        );
        assert_eq!(
            body(&e),
            r#"{"error":{"root_cause":[{"type":"query_shard_exception","reason":"failed to create query: x","index":"probe"}],"type":"search_phase_execution_exception","reason":"all shards failed","phase":"query","grouped":true,"failed_shards":[{"shard":0,"index":"probe","node":"n","reason":{"type":"query_shard_exception","reason":"failed to create query: x","index":"probe","caused_by":{"type":"number_format_exception","reason":"x"}}}],"caused_by":{"type":"query_shard_exception","reason":"failed to create query: x","index":"probe","caused_by":{"type":"number_format_exception","reason":"x"}}},"status":400}"#
        );
    }

    #[test]
    fn a_shard_failure_is_reported_as_all_shards_failed() {
        let e = SearchError::on_shard(
            400,
            Failure::plain("illegal_argument_exception", "too large"),
            ShardFailure {
                index: "i".into(),
                shard: 0,
                node: "n1".into(),
            },
        );
        assert_eq!(
            body(&e),
            r#"{"error":{"root_cause":[{"type":"illegal_argument_exception","reason":"too large"}],"type":"search_phase_execution_exception","reason":"all shards failed","phase":"query","grouped":true,"failed_shards":[{"shard":0,"index":"i","node":"n1","reason":{"type":"illegal_argument_exception","reason":"too large"}}],"caused_by":{"type":"illegal_argument_exception","reason":"too large","caused_by":{"type":"illegal_argument_exception","reason":"too large"}}},"status":400}"#
        );
    }
}
