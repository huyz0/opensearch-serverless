//! Why a mapping was refused, as OpenSearch reports it: the exception's type,
//! its message and what caused it, so a REST layer can answer with the
//! Java's error body. [`MappingError::unsupported`] is this node's own: a
//! valid mapping that the Rust writer does not index yet, which a router
//! sends to a Java writer instead.

use std::fmt;

/// A refused mapping, or a mapping this node cannot index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingError {
    /// The OpenSearch exception type, `mapper_parsing_exception` and so on,
    /// or [`UNSUPPORTED`].
    pub ty: &'static str,
    /// The message.
    pub reason: String,
    /// The exception this one wraps, as OpenSearch's `caused_by`.
    pub caused_by: Option<Box<MappingError>>,
}

/// The type of a mapping that is valid but not indexed by this node.
pub const UNSUPPORTED: &str = "unsupported_mapping";

impl MappingError {
    fn new(ty: &'static str, reason: &str) -> Self {
        Self {
            ty,
            // ALLOC: error path; one message.
            reason: reason.to_owned(),
            caused_by: None,
        }
    }

    /// `MapperParsingException`.
    pub fn mapper_parsing(reason: &str) -> Self {
        Self::new("mapper_parsing_exception", reason)
    }

    /// `IllegalArgumentException`.
    pub fn illegal_argument(reason: &str) -> Self {
        Self::new("illegal_argument_exception", reason)
    }

    /// `MapperException`, which OpenSearch answers with a 500.
    pub fn mapper(reason: &str) -> Self {
        Self::new("mapper_exception", reason)
    }

    /// `NumberFormatException`.
    pub fn number_format(input: &str) -> Self {
        Self::new(
            "number_format_exception",
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            &format!("For input string: \"{input}\""),
        )
    }

    /// `ArithmeticException`.
    pub fn arithmetic(reason: &str) -> Self {
        Self::new("arithmetic_exception", reason)
    }

    /// `StrictDynamicMappingException`.
    pub fn strict_dynamic(field: &str, within: &str) -> Self {
        Self::new(
            "strict_dynamic_mapping_exception",
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            &format!(
                "mapping set to strict, dynamic introduction of [{field}] within [{within}] is not allowed"
            ),
        )
    }

    /// Malformed JSON in a mapping (`parse_exception`).
    pub fn parse(reason: &str) -> Self {
        Self::new("parse_exception", reason)
    }

    /// A valid mapping this node does not index yet.
    pub fn unsupported(reason: &str) -> Self {
        Self::new(UNSUPPORTED, reason)
    }

    /// This error, caused by `cause`.
    pub fn caused_by(mut self, cause: MappingError) -> Self {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        self.caused_by = Some(Box::new(cause));
        self
    }

    /// Whether this is [`MappingError::unsupported`].
    pub fn is_unsupported(&self) -> bool {
        self.ty == UNSUPPORTED || self.caused_by.as_ref().is_some_and(|c| c.is_unsupported())
    }

    /// Wraps an error the way `MapperService` reports a mapping it could not
    /// parse: `Failed to parse mapping [_doc]: <reason>`, caused by the
    /// error. An unsupported mapping stays as it is.
    pub fn failed_to_parse(self) -> Self {
        if self.is_unsupported() {
            return self;
        }
        Self {
            ty: "mapper_parsing_exception",
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            reason: format!("Failed to parse mapping [_doc]: {}", self.reason),
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            caused_by: Some(Box::new(self)),
        }
    }
}

impl fmt::Display for MappingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.ty, self.reason)
    }
}

impl std::error::Error for MappingError {}
