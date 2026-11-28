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

/// The internal type of a `java.text.ParseException`; it renders as `parse_exception`.
pub const JAVA_PARSE: &str = "java_parse_exception";

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

    /// `ValidationException`, which the k-NN plugin throws for a method it checks.
    pub fn validation(reason: &str) -> Self {
        Self::new("validation_exception", reason)
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

    /// `StrictDynamicMappingException` for `dynamic: strict`.
    pub fn strict_dynamic(field: &str, within: &str) -> Self {
        Self::strict_dynamic_as("strict", within, field)
    }

    /// `StrictDynamicMappingException(mode, within, field)`, for the
    /// `dynamic` setting `mode`.
    pub fn strict_dynamic_as(mode: &str, within: &str, field: &str) -> Self {
        Self::new(
            "strict_dynamic_mapping_exception",
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            &format!(
                "mapping set to {mode}, dynamic introduction of [{field}] within [{within}] is not allowed"
            ),
        )
    }

    /// Malformed JSON in a document (Jackson's `JsonParseException`).
    pub fn json_parse(reason: &str) -> Self {
        Self::new("json_parse_exception", reason)
    }

    /// `NotXContentException`: a body that is no JSON object.
    pub fn not_x_content(reason: &str) -> Self {
        Self::new("not_x_content_exception", reason)
    }

    /// Jackson's `StreamReadException`, the cause Jackson's parse errors carry.
    pub fn stream_read(reason: &str) -> Self {
        Self::new("stream_read_exception", reason)
    }

    /// Jackson's `InputCoercionException`: a number accessor on a token
    /// that is not one, or a number out of the accessor's range.
    pub fn input_coercion(reason: &str) -> Self {
        Self::new("input_coercion_exception", reason)
    }

    /// A failed module call, typed by what a caller can do about it: 429
    /// `extension_busy` for capacity, 502 `extension_malformed` for a
    /// module that broke the contract, 400 `extension_failed` for a failure a
    /// retry would repeat. The write never falls back to a built-in analyzer.
    pub fn extension(failure: &oss_ext::CallError) -> Self {
        let ty = match failure.http_status() {
            429 => "extension_busy",
            502 => "extension_malformed",
            _ => "extension_failed",
        };
        // ALLOC: error path; one message.
        Self::new(
            ty,
            &format!(
                "the analyzer module failed ({:?}): {}",
                failure.kind(),
                failure.detail()
            ),
        )
    }

    /// `NullPointerException`, which the k-NN plugin throws for an encoder it does not know.
    pub fn null_pointer(reason: &str) -> Self {
        Self::new("null_pointer_exception", reason)
    }

    /// `IllegalStateException`.
    pub fn illegal_state(reason: &str) -> Self {
        Self::new("illegal_state_exception", reason)
    }

    /// `java.time.DateTimeException`.
    pub fn date_time(reason: &str) -> Self {
        Self::new("date_time_exception", reason)
    }

    /// `java.time.format.DateTimeParseException`.
    pub fn date_time_parse(reason: &str) -> Self {
        Self::new("date_time_parse_exception", reason)
    }

    /// OpenSearch's own `OpenSearchParseException`.
    pub fn opensearch_parse(reason: &str) -> Self {
        Self::new("parse_exception", reason)
    }

    /// `java.text.ParseException`, which renders as `parse_exception` but is
    /// not an `OpenSearchException`.
    pub fn java_parse(reason: &str) -> Self {
        Self::new(JAVA_PARSE, reason)
    }

    /// Malformed JSON in a mapping (`parse_exception`).
    pub fn parse(reason: &str) -> Self {
        Self::new("parse_exception", reason)
    }

    /// A valid mapping this node does not index yet.
    pub fn unsupported(reason: &str) -> Self {
        Self::new(UNSUPPORTED, reason)
    }

    /// This error, caused by `cause` when there is one.
    pub fn caused_by_opt(self, cause: Option<MappingError>) -> Self {
        match cause {
            Some(cause) => self.caused_by(cause),
            None => self,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use oss_ext::CallError;

    // docs/specs/extension-analyzer AC-2, AC-16
    #[test]
    fn a_failed_module_call_is_typed_by_what_a_caller_can_do() {
        let ty = |e: CallError| MappingError::extension(&e).ty;
        assert_eq!(ty(CallError::busy()), "extension_busy");
        assert_eq!(ty(CallError::malformed(9)), "extension_malformed");
        assert_eq!(ty(CallError::trap("unreachable")), "extension_failed");
        let e = MappingError::extension(&CallError::trap("unreachable"));
        assert!(e.reason.contains("unreachable"), "{}", e.reason);
    }
}
