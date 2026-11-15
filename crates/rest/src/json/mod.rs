//! A streaming JSON tokenizer.
//!
//! Bytes are fed in chunks as they arrive, and tokens come out through a
//! [`TokenSink`] as soon as they are complete. The tokenizer holds one fixed
//! scratch buffer and a container stack, both sized from [`Limits`] when it is
//! built, and never grows either: a key or number past its limit, or nesting
//! past the depth, is an error, and a string value of any length comes out in
//! pieces of at most [`Limits::string_chunk`] bytes. Nothing holds the whole
//! document, so a body is never parsed with `serde_json::from_slice`.
//!
//! The accepted language is what OpenSearch's Jackson factory accepts
//! (libs/x-content/src/main/java/org/opensearch/common/xcontent/json/JsonXContent.java
//! at huyz0/OpenSearch@49f238ed): RFC 8259 JSON plus `//` and `/* */`
//! comments, with its limits as the defaults (`XContentConstraints`: names
//! of 50,000, strings of 50,000,000, depth 1,000). Jackson counts those
//! limits in characters; this counts UTF-8 bytes. Duplicate keys, which
//! Jackson refuses, are the consumer's to detect. A lone UTF-16 surrogate in a
//! `\u` escape is refused, since the text must be valid UTF-8.

mod lexer;
mod number;
mod strings;

use std::fmt;

pub use lexer::Tokenizer;

/// One token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    /// `{`
    StartObject,
    /// `}`
    EndObject,
    /// `[`
    StartArray,
    /// `]`
    EndArray,
    /// An object key, unescaped and whole.
    Key(&'a str),
    /// A piece of a string value, unescaped. `last` marks its final piece; a
    /// short string is one piece.
    Str {
        /// The text.
        text: &'a str,
        /// Whether this is the string's last piece.
        last: bool,
    },
    /// A number, as written (validated against the JSON grammar).
    Number(&'a str),
    /// `true` or `false`.
    Bool(bool),
    /// `null`
    Null,
}

/// Receives tokens. An error stops the feed and is returned from it.
pub trait TokenSink {
    /// The sink's error; a [`JsonError`] must convert into it.
    type Error: From<JsonError>;

    /// Takes one token.
    fn token(&mut self, token: Token<'_>) -> Result<(), Self::Error>;

    /// Takes one token with where it starts: the opening quote of a key or
    /// string, the first byte of a number or literal, the bracket of a
    /// start or end. The tokenizer calls this; by default it drops the
    /// location and calls [`TokenSink::token`].
    fn token_at(&mut self, token: Token<'_>, at: Location) -> Result<(), Self::Error> {
        let _ = at;
        self.token(token)
    }
}

/// Where a token starts, as Jackson's `JsonLocation` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    /// The 1-based line.
    pub line: u64,
    /// The 1-based column, in bytes.
    pub column: u64,
}

/// The tokenizer's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The deepest nesting of objects and arrays.
    pub max_depth: usize,
    /// The longest key, in bytes.
    pub max_key_bytes: usize,
    /// The longest string value, in bytes.
    pub max_string_bytes: u64,
    /// The longest number, in bytes.
    pub max_number_bytes: usize,
    /// The largest piece a string value is handed on in (at least 4).
    pub string_chunk: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 1000,
            max_key_bytes: 50_000,
            max_string_bytes: 50_000_000,
            max_number_bytes: 1024,
            string_chunk: 16 << 10,
        }
    }
}

/// What went wrong, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// The kind of error.
    pub kind: ErrorKind,
    /// The byte offset in the document.
    pub offset: u64,
    /// The 1-based line.
    pub line: u64,
    /// The 1-based column, in bytes.
    pub column: u64,
}

/// The kinds of [`JsonError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A byte that cannot appear here.
    Unexpected(u8),
    /// The document ended inside a value or comment.
    UnexpectedEnd,
    /// Non-whitespace after the root value.
    Trailing(u8),
    /// A number that does not match the JSON grammar.
    BadNumber,
    /// A bad `\` escape.
    BadEscape,
    /// A lone or mismatched UTF-16 surrogate in a `\u` escape.
    LoneSurrogate,
    /// String bytes that are not UTF-8.
    InvalidUtf8,
    /// A raw control character inside a string.
    ControlInString(u8),
    /// Nesting deeper than [`Limits::max_depth`].
    TooDeep,
    /// A key longer than [`Limits::max_key_bytes`].
    KeyTooLong,
    /// A string longer than [`Limits::max_string_bytes`].
    StringTooLong,
    /// A number longer than [`Limits::max_number_bytes`].
    NumberTooLong,
    /// The limits are unusable (a string chunk under 4 bytes).
    BadLimits,
}

impl fmt::Display for JsonError {
    /// In the spirit of Jackson's messages: what, then where.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Unexpected(b) => write!(f, "Unexpected character ({}) ", show(b))?,
            ErrorKind::UnexpectedEnd => f.write_str("Unexpected end-of-input ")?,
            ErrorKind::Trailing(b) => write!(
                f,
                "Unexpected character ({}) after the root value ",
                show(b)
            )?,
            ErrorKind::BadNumber => f.write_str("Invalid numeric value ")?,
            ErrorKind::BadEscape => f.write_str("Unrecognized character escape ")?,
            ErrorKind::LoneSurrogate => f.write_str("Invalid UTF-16 surrogate in a \\u escape ")?,
            ErrorKind::InvalidUtf8 => f.write_str("Invalid UTF-8 ")?,
            ErrorKind::ControlInString(b) => {
                write!(
                    f,
                    "Illegal unquoted character ((CTRL-CHAR, code {b})): has to be escaped "
                )?;
            }
            ErrorKind::TooDeep => {
                f.write_str("Document nesting depth exceeds the maximum allowed ")?
            }
            ErrorKind::KeyTooLong => f.write_str("Name length exceeds the maximum allowed ")?,
            ErrorKind::StringTooLong => {
                f.write_str("String value length exceeds the maximum allowed ")?
            }
            ErrorKind::NumberTooLong => {
                f.write_str("Number value length exceeds the maximum allowed ")?
            }
            ErrorKind::BadLimits => f.write_str("Invalid tokenizer limits ")?,
        }
        write!(f, "at line: {}, column: {}", self.line, self.column)
    }
}

impl std::error::Error for JsonError {}

fn show(b: u8) -> String {
    // ALLOC: error path; one character's description.
    if b.is_ascii_graphic() {
        format!("'{}' (code {b})", char::from(b))
    } else {
        // ALLOC: error path; bounded by the request line or a fixed message.
        format!("code {b}")
    }
}

impl From<JsonError> for crate::error::RestError {
    /// 400 `json_parse_exception`.
    fn from(e: JsonError) -> Self {
        // ALLOC: error path; bounded by the request line or a fixed message.
        crate::error::RestError::json_parse(e.to_string())
    }
}

/// How a document ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// Nothing but whitespace and comments.
    Empty,
    /// One complete value.
    Value,
}

#[cfg(test)]
mod tests;
