//! Typed tokenizer errors, each carrying the absolute byte offset of the fault.

use std::fmt;

/// What was wrong with the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A byte that cannot appear here in JSON.
    UnexpectedByte(u8),
    /// The input ended inside a value, or before any value in single-document mode.
    UnexpectedEof,
    /// A number that breaks the JSON number grammar.
    InvalidNumber,
    /// A number longer than [`Config::max_number_len`](crate::Config::max_number_len).
    NumberTooLong,
    /// A backslash escape other than `\" \\ \/ \b \f \n \r \t \uXXXX`.
    InvalidEscape,
    /// A `\u` escape naming half of a UTF-16 surrogate pair without its other half.
    LoneSurrogate,
    /// String bytes that are not UTF-8.
    InvalidUtf8,
    /// An unescaped byte below 0x20 inside a string.
    ControlCharacter,
    /// Containers nested deeper than [`Config::max_depth`](crate::Config::max_depth).
    DepthExceeded,
    /// A line break inside a document in NDJSON framing.
    NewlineInDocument,
    /// Bytes after the document where none may follow.
    TrailingData,
}

/// A tokenizer error: what went wrong, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    offset: u64,
    line: u64,
    column: u64,
}

impl Error {
    /// An error with no line or column (both 0), for tests.
    #[cfg(test)]
    pub(crate) fn new(kind: ErrorKind, offset: u64) -> Self {
        Self::at(kind, offset, 0, 0)
    }

    /// An error at `offset`, on `line` at `column`.
    pub(crate) fn at(kind: ErrorKind, offset: u64, line: u64, column: u64) -> Self {
        Self {
            kind,
            offset,
            line,
            column,
        }
    }

    /// The 1-based line of the byte at fault.
    pub fn line(&self) -> u64 {
        self.line
    }

    /// The 1-based column, in bytes, of the byte at fault.
    pub fn column(&self) -> u64 {
        self.column
    }

    /// What went wrong.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Absolute offset, from the first byte ever fed, of the byte at fault
    /// (or of the end of input, for [`ErrorKind::UnexpectedEof`]).
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            ErrorKind::UnexpectedByte(b) => {
                return write!(f, "unexpected byte 0x{b:02x} at offset {}", self.offset);
            }
            ErrorKind::UnexpectedEof => "unexpected end of input",
            ErrorKind::InvalidNumber => "invalid number",
            ErrorKind::NumberTooLong => "number too long",
            ErrorKind::InvalidEscape => "invalid escape",
            ErrorKind::LoneSurrogate => "lone UTF-16 surrogate in escape",
            ErrorKind::InvalidUtf8 => "invalid UTF-8 in string",
            ErrorKind::ControlCharacter => "control character in string",
            ErrorKind::DepthExceeded => "nesting too deep",
            ErrorKind::NewlineInDocument => "line break inside an NDJSON document",
            ErrorKind::TrailingData => "trailing data after document",
        };
        write!(f, "{what} at offset {}", self.offset)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_kind_and_offset() {
        assert_eq!(
            Error::new(ErrorKind::UnexpectedByte(b'x'), 3).to_string(),
            "unexpected byte 0x78 at offset 3"
        );
        let all = [
            (ErrorKind::UnexpectedEof, "unexpected end of input"),
            (ErrorKind::InvalidNumber, "invalid number"),
            (ErrorKind::NumberTooLong, "number too long"),
            (ErrorKind::InvalidEscape, "invalid escape"),
            (ErrorKind::LoneSurrogate, "lone UTF-16 surrogate in escape"),
            (ErrorKind::InvalidUtf8, "invalid UTF-8 in string"),
            (ErrorKind::ControlCharacter, "control character in string"),
            (ErrorKind::DepthExceeded, "nesting too deep"),
            (
                ErrorKind::NewlineInDocument,
                "line break inside an NDJSON document",
            ),
            (ErrorKind::TrailingData, "trailing data after document"),
        ];
        for (kind, text) in all {
            let e = Error::new(kind, 9);
            assert_eq!(e.kind(), kind);
            assert_eq!(e.offset(), 9);
            assert_eq!(e.to_string(), format!("{text} at offset 9"));
        }
    }
}
