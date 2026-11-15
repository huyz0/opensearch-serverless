//! The events a tokenizer yields.

/// One fragment of a string or key. A string arrives as one or more fragments:
/// the first has `first` set, the last has `last` set (one fragment may be both).
/// Escapes are already decoded. Concatenating the fragments gives the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Text<'a> {
    /// The decoded text of this fragment; may be empty.
    pub fragment: &'a str,
    /// This is the string's first fragment.
    pub first: bool,
    /// This is the string's last fragment.
    pub last: bool,
}

/// One tokenizer step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// `{`
    BeginObject,
    /// `}`
    EndObject,
    /// `[`
    BeginArray,
    /// `]`
    EndArray,
    /// A fragment of an object key.
    Key(Text<'a>),
    /// A fragment of a string value.
    String(Text<'a>),
    /// A number, as its JSON text (grammar-checked, not converted).
    Number(&'a str),
    /// `true` or `false`.
    Bool(bool),
    /// `null`
    Null,
    /// A top-level document ended. `start` is the absolute offset of its first
    /// byte and `end` one past its last, so `start..end` is its raw text.
    EndDocument {
        /// Offset of the document's first byte.
        start: u64,
        /// Offset one past the document's last byte.
        end: u64,
    },
    /// The input slice is used up; call again with more bytes, or with `eof`.
    NeedMore,
    /// The input is finished and complete. Every later call returns `Eof` again.
    Eof,
}
