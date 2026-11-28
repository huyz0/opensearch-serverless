//! A refusal, in the shape of the plugin's error body.

use serde_json::{Value, json};

/// Why a statement or a result was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    status: u16,
    kind: &'static str,
    reason: &'static str,
    details: String,
}

impl Error {
    /// A statement that does not parse: `offending` is the token and `pos` its byte position.
    #[must_use]
    pub fn syntax(offending: &str, pos: usize) -> Self {
        let (kind, details) = if offending == "<EOF>" {
            ("EOFParserException", "EOF".to_owned())
        } else {
            // ALLOC: one message, bounded by the identifier and token limits.
            let end = pos.saturating_add(offending.len());
            (
                "ParserException",
                format!("ERROR. token : {}, pos : {end}", offending.to_uppercase()),
            )
        };
        Self {
            status: 400,
            kind,
            reason: "Invalid SQL query",
            details,
        }
    }

    /// A comparison of two types the plugin has no function for.
    #[must_use]
    pub fn type_mismatch(func: &str, left: &str, right: &str) -> Self {
        const SIGNATURES: &str = "[BYTE,BYTE],[SHORT,SHORT],[INTEGER,INTEGER],[LONG,LONG],[FLOAT,FLOAT],[DOUBLE,DOUBLE],[STRING,STRING],[BOOLEAN,BOOLEAN],[DATE,DATE],[TIME,TIME],[TIMESTAMP,TIMESTAMP],[INTERVAL,INTERVAL],[IP,IP],[GEO_POINT,GEO_POINT],[BINARY,BINARY],[STRUCT,STRUCT],[ARRAY,ARRAY]";
        Self {
            status: 400,
            kind: "ExpressionEvaluationException",
            reason: "Invalid SQL query",
            // ALLOC: one message, bounded by the type names.
            details: format!("{func} function expected {{{SIGNATURES}}}, but got [{left},{right}]"),
        }
    }

    /// An empty statement.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            status: 400,
            kind: "IllegalArgumentException",
            reason: "Invalid SQL query",
            details: "Query is empty".to_owned(),
        }
    }

    /// A statement that is not a `SELECT`, `SHOW` or `DESCRIBE`.
    #[must_use]
    pub fn not_a_query(sql: &str) -> Self {
        Self {
            status: 400,
            kind: "SQLFeatureNotSupportedException",
            reason: "Invalid SQL query",
            // ALLOC: the statement again, bounded by the statement byte limit.
            details: format!("Query must start with SELECT, DELETE, SHOW or DESCRIBE: {sql}"),
        }
    }

    /// A statement bound that was passed (bytes, tokens, depth, identifier length).
    #[must_use]
    pub fn bound(what: &str, limit: usize) -> Self {
        Self {
            status: 400,
            kind: "SyntaxCheckException",
            reason: "Invalid SQL query",
            // ALLOC: one message of fixed size.
            details: format!("The statement is past its bound: {what} (at most {limit})"),
        }
    }

    /// A construct the planner cannot push into a search body.
    #[must_use]
    pub fn unsupported(details: impl Into<String>) -> Self {
        Self {
            status: 400,
            kind: "UnsupportedOperationException",
            reason: "Invalid SQL query",
            details: details.into(),
        }
    }

    /// A name or type the statement uses that the mapping does not have.
    #[must_use]
    pub fn semantic(details: impl Into<String>) -> Self {
        Self {
            status: 400,
            kind: "SemanticCheckException",
            reason: "Invalid SQL query",
            details: details.into(),
        }
    }

    /// A result that does not fit its byte bound.
    #[must_use]
    pub fn too_large(limit: usize) -> Self {
        Self {
            status: 429,
            kind: "CircuitBreakingException",
            reason: "Result too large",
            // ALLOC: one message of fixed size.
            details: format!(
                "The result is past its bound of {limit} bytes; add a LIMIT or a filter"
            ),
        }
    }

    /// The HTTP status.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The text that says what was refused.
    #[must_use]
    pub fn details(&self) -> &str {
        &self.details
    }

    /// The plugin's JSON error body.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"error": {"reason": self.reason, "details": self.details, "type": self.kind}, "status": self.status})
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.details)
    }
}

impl std::error::Error for Error {}
