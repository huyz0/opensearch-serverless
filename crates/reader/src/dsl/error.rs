//! Why a search body could not be parsed, as OpenSearch's exceptions.

use oss_rest::json::{JsonError, Location};

use crate::failure::{Failure, Meta};

/// A parse failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DslError {
    /// The body is not JSON (Jackson's `JsonParseException`).
    Json(JsonError),
    /// The body's tokens would need more than `limit` bytes to hold.
    TooLarge {
        /// The limit.
        limit: usize,
    },
    /// One of OpenSearch's parse exceptions.
    Failure(Failure),
}

impl DslError {
    /// `new ParsingException(location, reason)`.
    pub fn parsing(at: Location, reason: impl Into<String>) -> Self {
        DslError::Failure(parsing_failure(at, reason))
    }

    /// `new XContentParseException(location, reason, cause)`: the location
    /// leads the message.
    pub fn xcontent(at: Location, reason: &str, cause: Option<DslError>) -> Self {
        // ALLOC: error path; one message.
        let f = Failure::plain(
            "x_content_parse_exception",
            format!("[{}:{}] {reason}", at.line, at.column),
        );
        DslError::Failure(match cause {
            Some(c) => f.caused_by(c.into_failure()),
            None => f,
        })
    }

    /// `new IllegalArgumentException(reason)`.
    pub fn illegal(reason: impl Into<String>) -> Self {
        DslError::Failure(Failure::plain("illegal_argument_exception", reason))
    }

    /// A part of OpenSearch's DSL this reader does not serve yet. OpenSearch
    /// would accept it; the reader refuses it rather than answer differently.
    pub fn unsupported(what: &str) -> Self {
        // ALLOC: error path; one message.
        DslError::illegal(format!("{what} is not supported by this node yet"))
    }

    /// The HTTP status OpenSearch answers this failure with: 500 for an
    /// `AggregationInitializationException`, which keeps
    /// `OpenSearchException.status()`'s default, else 400.
    pub fn status(&self) -> u16 {
        match self {
            DslError::Failure(f) if f.kind == "aggregation_initialization_exception" => 500,
            DslError::Failure(f) if f.kind == "resource_not_found_exception" => 404,
            DslError::Failure(f) if f.kind == "circuit_breaking_exception" => 429,
            DslError::Failure(f) if f.kind == "exception" => 500,
            _ => 400,
        }
    }

    /// The exception to answer with.
    pub fn into_failure(self) -> Failure {
        match self {
            // ALLOC: error path; one message.
            DslError::Json(e) => Failure::plain("json_parse_exception", e.to_string()),
            DslError::TooLarge { limit } => Failure::plain(
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!("the search request needs more than [{limit}] bytes to parse"),
            ),
            DslError::Failure(f) => f,
        }
    }
}

/// A `ParsingException`'s exception: `line` and `col` after the reason.
pub fn parsing_failure(at: Location, reason: impl Into<String>) -> Failure {
    Failure::opensearch("parsing_exception", reason)
        .with_meta("line", Meta::Int(at.line))
        .with_meta("col", Meta::Int(at.column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_rest::json::ErrorKind;

    #[test]
    fn each_kind_becomes_its_opensearch_exception() {
        let at = Location {
            line: 1,
            column: 19,
        };
        let f = DslError::xcontent(at, "[bool] unknown field [nope]", None).into_failure();
        assert_eq!(
            (f.kind, f.reason.as_str()),
            (
                "x_content_parse_exception",
                "[1:19] [bool] unknown field [nope]"
            )
        );
        let wrapped = DslError::xcontent(
            at,
            "[bool] failed to parse field [must]",
            Some(DslError::parsing(at, "unknown query [x]")),
        )
        .into_failure();
        assert_eq!(
            wrapped.cause.as_deref().map(|c| c.kind),
            Some("parsing_exception")
        );
        let p = DslError::parsing(at, "r").into_failure();
        assert!(p.opensearch);
        assert_eq!(p.meta, [("line", Meta::Int(1)), ("col", Meta::Int(19))]);
        let j = DslError::Json(JsonError {
            kind: ErrorKind::UnexpectedEnd,
            offset: 3,
            line: 1,
            column: 4,
        })
        .into_failure();
        assert_eq!(j.kind, "json_parse_exception");
        assert!(j.reason.contains("line: 1, column: 4"), "{}", j.reason);
        let t = DslError::TooLarge { limit: 9 }.into_failure();
        assert!(t.reason.contains("[9]"));
        let u = DslError::unsupported("[x]").into_failure();
        assert_eq!(u.reason, "[x] is not supported by this node yet");
    }
}
