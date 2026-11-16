//! A pull reader over JSON bytes, for the shell's JSON records.
//!
//! The shell writes its segment blob headers, commit manifests and index
//! descriptors with OpenSearch's `XContentBuilder` and reads them with an
//! `XContentParser`, token by token. This is the same token loop over a byte
//! slice: strings borrow from the input unless they carry an escape, numbers
//! are read as integers, and nesting deeper than [`MAX_DEPTH`] is refused, so
//! skipping an unknown field with [`Reader::skip_value`] copies nothing and a hostile record cannot
//! recurse without bound.

use std::borrow::Cow;

use crate::error::FormatError;

pub(crate) mod out;
mod strings;

/// The deepest nesting [`Reader::skip_after`] follows. The shell's records
/// nest four or five levels; a mapping inside a descriptor is a string.
pub(crate) const MAX_DEPTH: usize = 64;

/// One JSON token.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    /// `{`
    StartObject,
    /// `}`
    EndObject,
    /// `[`
    StartArray,
    /// `]`
    EndArray,
    /// An object's field name.
    Field(Cow<'a, str>),
    /// A string value.
    Str(Cow<'a, str>),
    /// An integer that fits an `i64`.
    Int(i64),
    /// Any other number: a fraction, an exponent, or an integer past `i64`.
    OtherNumber,
    /// `true` or `false`.
    Bool(bool),
    /// `null`
    Null,
}

/// What the reader expects next inside a container.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A value, at the top level.
    Value,
    /// The first field of an object, or its close.
    FirstField,
    /// A comma then a field, or the object's close.
    NextField,
    /// The value after a field name.
    FieldValue,
    /// The first element of an array, or its close.
    FirstElement,
    /// A comma then an element, or the array's close.
    NextElement,
    /// Nothing more: the top-level value is complete.
    End,
}

/// A pull reader over one JSON document.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
    // One bit per open container: set for an object. Bounded by MAX_DEPTH.
    stack: u64,
    depth: usize,
    expect: Expect,
    // False while a value is being skipped: an escaped string is then checked
    // but not decoded, so skipping copies nothing.
    decode: bool,
    // Where the last number read starts and ends, for its text.
    number: (usize, usize),
}

impl<'a> Reader<'a> {
    /// A reader over `bytes`, naming `what` it reads in its errors.
    pub(crate) fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self {
            bytes,
            at: 0,
            what,
            stack: 0,
            depth: 0,
            expect: Expect::Value,
            decode: true,
            number: (0, 0),
        }
    }

    fn corrupt(&self, reason: &'static str) -> FormatError {
        FormatError::Corrupt {
            what: self.what,
            reason,
        }
    }

    fn truncated(&self) -> FormatError {
        FormatError::Corrupt {
            what: self.what,
            reason: "the JSON ends early",
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.bytes.get(self.at) {
            if !matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                break;
            }
            self.at = self.at.saturating_add(1);
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_whitespace();
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// Whether the innermost open container is an object.
    fn in_object(&self) -> bool {
        self.depth
            .checked_sub(1)
            .and_then(|d| self.stack.checked_shr(u32::try_from(d).ok()?))
            .is_some_and(|bits| bits & 1 == 1)
    }

    fn push(&mut self, object: bool) -> Result<(), FormatError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.corrupt("the JSON nests too deeply"));
        }
        let bit = 1u64 << self.depth;
        if object {
            self.stack |= bit;
        } else {
            self.stack &= !bit;
        }
        self.depth = self.depth.saturating_add(1);
        self.expect = if object {
            Expect::FirstField
        } else {
            Expect::FirstElement
        };
        Ok(())
    }

    fn pop(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.expect = self.after_value();
    }

    fn after_value(&self) -> Expect {
        if self.depth == 0 {
            Expect::End
        } else if self.in_object() {
            Expect::NextField
        } else {
            Expect::NextElement
        }
    }

    /// The next token, or `None` once the document is complete and only
    /// whitespace follows.
    pub(crate) fn next(&mut self) -> Result<Option<Token<'a>>, FormatError> {
        match self.expect {
            Expect::End => {
                return match self.peek() {
                    None => Ok(None),
                    Some(_) => Err(self.corrupt("content follows the JSON value")),
                };
            }
            Expect::FirstField | Expect::NextField => {
                let Some(b) = self.peek() else {
                    return Err(self.truncated());
                };
                if b == b'}' {
                    self.bump();
                    self.pop();
                    return Ok(Some(Token::EndObject));
                }
                if self.expect == Expect::NextField {
                    if b != b',' {
                        return Err(self.corrupt("expected a comma between fields"));
                    }
                    self.bump();
                }
                if self.peek() != Some(b'"') {
                    return Err(self.corrupt("expected a field name"));
                }
                self.bump();
                let name = self.string()?;
                if self.peek() != Some(b':') {
                    return Err(self.corrupt("expected a colon after a field name"));
                }
                self.bump();
                self.expect = Expect::FieldValue;
                return Ok(Some(Token::Field(name)));
            }
            Expect::FirstElement | Expect::NextElement => {
                let Some(b) = self.peek() else {
                    return Err(self.truncated());
                };
                if b == b']' {
                    self.bump();
                    self.pop();
                    return Ok(Some(Token::EndArray));
                }
                if self.expect == Expect::NextElement {
                    if b != b',' {
                        return Err(self.corrupt("expected a comma between elements"));
                    }
                    self.bump();
                }
            }
            Expect::Value | Expect::FieldValue => {}
        }
        self.value().map(Some)
    }

    fn value(&mut self) -> Result<Token<'a>, FormatError> {
        let Some(b) = self.peek() else {
            return Err(self.truncated());
        };
        let token = match b {
            b'{' => {
                self.bump();
                self.push(true)?;
                return Ok(Token::StartObject);
            }
            b'[' => {
                self.bump();
                self.push(false)?;
                return Ok(Token::StartArray);
            }
            b'"' => {
                self.bump();
                Token::Str(self.string()?)
            }
            b't' => self.literal(b"true", Token::Bool(true))?,
            b'f' => self.literal(b"false", Token::Bool(false))?,
            b'n' => self.literal(b"null", Token::Null)?,
            b'-' | b'0'..=b'9' => self.number()?,
            _ => return Err(self.corrupt("expected a JSON value")),
        };
        self.expect = self.after_value();
        Ok(token)
    }

    fn literal(&mut self, word: &[u8], token: Token<'a>) -> Result<Token<'a>, FormatError> {
        let end = self.at.saturating_add(word.len());
        match self.bytes.get(self.at..end) {
            Some(found) if found == word => {
                self.at = end;
                Ok(token)
            }
            Some(_) => Err(self.corrupt("expected a JSON value")),
            None => Err(self.truncated()),
        }
    }

    fn number(&mut self) -> Result<Token<'a>, FormatError> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.bump();
        }
        let digits = self.at;
        while matches!(self.bytes.get(self.at), Some(b'0'..=b'9')) {
            self.bump();
        }
        if self.at == digits {
            return Err(self.corrupt("a number has no digits"));
        }
        if self.bytes.get(digits) == Some(&b'0') && self.at.saturating_sub(digits) > 1 {
            return Err(self.corrupt("a number has a leading zero"));
        }
        let integer_end = self.at;
        if self.bytes.get(self.at) == Some(&b'.') {
            self.bump();
            let fraction = self.at;
            while matches!(self.bytes.get(self.at), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.at == fraction {
                return Err(self.corrupt("a number's fraction has no digits"));
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.bump();
            }
            let exponent = self.at;
            while matches!(self.bytes.get(self.at), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.at == exponent {
                return Err(self.corrupt("a number's exponent has no digits"));
            }
        }
        self.number = (start, self.at);
        if self.at != integer_end {
            return Ok(Token::OtherNumber);
        }
        let text = self
            .bytes
            .get(start..integer_end)
            .and_then(|t| std::str::from_utf8(t).ok());
        Ok(text
            .and_then(|t| t.parse::<i64>().ok())
            .map_or(Token::OtherNumber, Token::Int))
    }

    /// The text of the last number read, as it is in the document: what
    /// Jackson's `getText` answers for a number.
    pub(crate) fn number_text(&self) -> &'a str {
        let (start, end) = self.number;
        self.bytes
            .get(start..end)
            .and_then(|t| std::str::from_utf8(t).ok())
            .unwrap_or_default()
    }

    /// Whether the next value opens an object, without reading it.
    pub(crate) fn next_is_object(&mut self) -> bool {
        self.peek() == Some(b'{')
    }

    /// Skips the next value, a field's or an element's, whole and without
    /// decoding any string in it.
    pub(crate) fn skip_value(&mut self) -> Result<(), FormatError> {
        self.decode = false;
        let skipped = self
            .expect_token()
            .and_then(|first| self.skip_after(&first));
        self.decode = true;
        skipped
    }

    /// Skips the next value, which must be a string or null, and returns
    /// the string's bytes between its quotes, escapes left in: nothing is
    /// decoded or copied. `None` for null.
    pub(crate) fn raw_string(&mut self) -> Result<Option<&'a [u8]>, FormatError> {
        let start = match self.peek() {
            Some(b'"') => self.at.saturating_add(1),
            Some(b'n') => {
                self.skip_value()?;
                return Ok(None);
            }
            _ => return Err(self.corrupt("expected a string")),
        };
        self.skip_value()?;
        // The closing quote is the byte before the reader's position.
        let end = self.at.saturating_sub(1);
        Ok(self.bytes.get(start..end))
    }

    /// Skips the next value, which must be an object, and returns its bytes
    /// from `{` to `}`: nothing is decoded or copied.
    pub(crate) fn raw_object(&mut self, reason: &'static str) -> Result<&'a [u8], FormatError> {
        if self.peek() != Some(b'{') {
            return Err(self.corrupt(reason));
        }
        let start = self.at;
        self.skip_value()?;
        Ok(self.bytes.get(start..self.at).unwrap_or_default())
    }

    /// Skips the value whose first token was `first`: nothing more for a
    /// scalar, everything up to the matching close for a container.
    pub(crate) fn skip_after(&mut self, first: &Token<'a>) -> Result<(), FormatError> {
        let mut open = match first {
            Token::StartObject | Token::StartArray => 1usize,
            _ => return Ok(()),
        };
        while open > 0 {
            match self.next()? {
                Some(Token::StartObject | Token::StartArray) => open = open.saturating_add(1),
                Some(Token::EndObject | Token::EndArray) => open = open.saturating_sub(1),
                Some(_) => {}
                None => return Err(self.truncated()),
            }
        }
        Ok(())
    }

    /// The next token, which must exist.
    pub(crate) fn expect_token(&mut self) -> Result<Token<'a>, FormatError> {
        self.next()?.ok_or_else(|| self.truncated())
    }

    /// Requires the next token to open an object.
    pub(crate) fn start_object(&mut self, reason: &'static str) -> Result<(), FormatError> {
        match self.expect_token()? {
            Token::StartObject => Ok(()),
            _ => Err(self.corrupt(reason)),
        }
    }

    /// Requires that nothing but whitespace follows the document. Called
    /// once the top-level value has closed, where [`Reader::next`] refuses
    /// anything else.
    pub(crate) fn finish(&mut self) -> Result<(), FormatError> {
        self.next().map(drop)
    }

    /// The next field name of the open object, or `None` at its close.
    pub(crate) fn next_field(&mut self) -> Result<Option<Cow<'a, str>>, FormatError> {
        match self.expect_token()? {
            Token::EndObject => Ok(None),
            Token::Field(name) => Ok(Some(name)),
            _ => Err(self.corrupt("expected a field")),
        }
    }

    /// A field-shaped error naming this reader's record.
    pub(crate) fn error(&self, reason: &'static str) -> FormatError {
        self.corrupt(reason)
    }
}
