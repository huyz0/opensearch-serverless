//! Tokenizer configuration.

/// How top-level documents are framed in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// Exactly one document, with optional surrounding whitespace.
    Single,
    /// Zero or more documents, separated by optional whitespace; a document may
    /// span lines (pretty-printed JSON).
    Concatenated,
    /// Zero or more documents, one per line, as OpenSearch's `_bulk` expects. A
    /// line break inside a document is refused, a second document on the same
    /// line is refused, and blank lines and `\r\n` are allowed.
    Ndjson,
}

/// Limits and framing. Every limit bounds memory the tokenizer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// How documents are framed.
    pub framing: Framing,
    /// Deepest nesting of arrays and objects accepted. Default 128 (serde_json accepts 127).
    pub max_depth: usize,
    /// Longest number accepted, in bytes. Numbers are the only token buffered
    /// whole; strings stream. Default 1,024.
    pub max_number_len: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            framing: Framing::Single,
            max_depth: 128,
            max_number_len: 1024,
        }
    }
}

impl Config {
    /// The default limits with the given framing.
    pub fn with_framing(framing: Framing) -> Self {
        Self {
            framing,
            ..Self::default()
        }
    }
}
