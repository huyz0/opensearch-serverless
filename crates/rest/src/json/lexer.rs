//! The tokenizer's state machine.

use super::number;
use super::{Completion, ErrorKind, JsonError, Limits, Token, TokenSink};

/// What the grammar expects next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Expect {
    Value,
    ValueOrEnd,
    KeyOrEnd,
    Key,
    Colon,
    CommaOrEnd,
    Done,
}

/// Which token, if any, the lexer is inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lex {
    Between,
    Str,
    Escape,
    Unicode,
    Number,
    Literal,
    Slash,
    LineComment,
    BlockComment,
    BlockStar,
}

/// A streaming JSON tokenizer for one document; see the module docs.
#[derive(Debug)]
pub struct Tokenizer {
    pub(super) limits: Limits,
    pub(super) stack: Vec<bool>,
    pub(super) scratch: Vec<u8>,
    pub(super) expect: Expect,
    pub(super) lex: Lex,
    pub(super) offset: u64,
    pub(super) line: u64,
    pub(super) line_start: u64,
    pub(super) in_key: bool,
    pub(super) string_len: u64,
    pub(super) unicode: u32,
    pub(super) unicode_digits: u8,
    pub(super) high: Option<u32>,
    pub(super) literal: &'static [u8],
    pub(super) literal_pos: usize,
    pub(super) root_seen: bool,
    pub(super) failed: Option<JsonError>,
}

impl Tokenizer {
    /// A tokenizer with `limits`, allocating its stack and scratch once.
    pub fn new(limits: Limits) -> Self {
        let cap = limits
            .max_key_bytes
            .max(limits.max_number_bytes)
            .max(limits.string_chunk);
        let failed = bad_limits(&limits);
        Self {
            limits,
            // ALLOC: once per tokenizer; `max_depth` entries.
            stack: Vec::with_capacity(limits.max_depth),
            // ALLOC: once per tokenizer; the largest key, number or string piece.
            scratch: Vec::with_capacity(cap),
            expect: Expect::Value,
            lex: Lex::Between,
            offset: 0,
            line: 1,
            line_start: 0,
            in_key: false,
            string_len: 0,
            unicode: 0,
            unicode_digits: 0,
            high: None,
            literal: b"",
            literal_pos: 0,
            root_seen: false,
            failed,
        }
    }

    /// Readies the tokenizer for a new document, keeping its allocations.
    pub fn reset(&mut self) {
        self.stack.clear();
        self.scratch.clear();
        self.expect = Expect::Value;
        self.lex = Lex::Between;
        self.offset = 0;
        self.line = 1;
        self.line_start = 0;
        self.in_key = false;
        self.string_len = 0;
        self.unicode = 0;
        self.unicode_digits = 0;
        self.high = None;
        self.literal = b"";
        self.literal_pos = 0;
        self.root_seen = false;
        self.failed = bad_limits(&self.limits);
    }

    /// Bytes fed so far.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Feeds the next chunk, handing complete tokens to `sink`.
    pub fn feed<S: TokenSink>(&mut self, chunk: &[u8], sink: &mut S) -> Result<(), S::Error> {
        if let Some(e) = self.failed {
            return Err(e.into());
        }
        let result = self.run(chunk, sink);
        self.offset = self
            .offset
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        result
    }

    /// Ends the document: `Empty` for whitespace and comments only, `Value`
    /// for one complete value, an error for anything unfinished.
    pub fn finish<S: TokenSink>(&mut self, sink: &mut S) -> Result<Completion, S::Error> {
        if let Some(e) = self.failed {
            return Err(e.into());
        }
        match self.lex {
            Lex::Number => self.end_number(self.offset, sink)?,
            Lex::Between | Lex::LineComment => {}
            _ => return Err(self.fail(ErrorKind::UnexpectedEnd, self.offset).into()),
        }
        self.lex = Lex::Between;
        match self.expect {
            Expect::Done => Ok(Completion::Value),
            Expect::Value if !self.root_seen && self.stack.is_empty() => Ok(Completion::Empty),
            _ => Err(self.fail(ErrorKind::UnexpectedEnd, self.offset).into()),
        }
    }

    fn at_kind(&self, kind: ErrorKind, at: u64) -> JsonError {
        JsonError {
            kind,
            offset: at,
            line: self.line,
            column: at.saturating_sub(self.line_start).saturating_add(1),
        }
    }

    pub(super) fn fail(&mut self, kind: ErrorKind, at: u64) -> JsonError {
        let e = self.at_kind(kind, at);
        self.failed = Some(e);
        e
    }

    fn run<S: TokenSink>(&mut self, chunk: &[u8], sink: &mut S) -> Result<(), S::Error> {
        let mut i = 0;
        while let Some(&b) = chunk.get(i) {
            let at = self
                .offset
                .saturating_add(u64::try_from(i).unwrap_or(u64::MAX));
            match self.lex {
                Lex::Between => {
                    match b {
                        b' ' | b'\t' | b'\r' => {}
                        b'\n' => self.newline(at),
                        b'/' => self.lex = Lex::Slash,
                        _ => self.structural(b, at, sink)?,
                    }
                    i = i.saturating_add(1);
                }
                Lex::Str => {
                    let rest = &chunk[i..];
                    let run = rest
                        .iter()
                        .position(|&c| c == b'"' || c == b'\\' || c < 0x20)
                        .unwrap_or(rest.len());
                    if run > 0 && self.high.is_some() {
                        return Err(self.fail(ErrorKind::LoneSurrogate, at).into());
                    }
                    self.push_str(&rest[..run], at, sink)?;
                    i = i.saturating_add(run);
                    let Some(&c) = chunk.get(i) else { break };
                    let at = self
                        .offset
                        .saturating_add(u64::try_from(i).unwrap_or(u64::MAX));
                    match c {
                        b'"' if self.high.is_some() => {
                            return Err(self.fail(ErrorKind::LoneSurrogate, at).into());
                        }
                        b'"' => self.end_string(at, sink)?,
                        b'\\' => self.lex = Lex::Escape,
                        _ => return Err(self.fail(ErrorKind::ControlInString(c), at).into()),
                    }
                    i = i.saturating_add(1);
                }
                Lex::Escape => {
                    self.escape(b, at, sink)?;
                    i = i.saturating_add(1);
                }
                Lex::Unicode => {
                    self.unicode_digit(b, at, sink)?;
                    i = i.saturating_add(1);
                }
                Lex::Number => {
                    let rest = &chunk[i..];
                    let run = rest
                        .iter()
                        .position(|&c| !number::continues(c))
                        .unwrap_or(rest.len());
                    if self.scratch.len().saturating_add(run) > self.limits.max_number_bytes {
                        return Err(self.fail(ErrorKind::NumberTooLong, at).into());
                    }
                    self.scratch.extend_from_slice(&rest[..run]);
                    i = i.saturating_add(run);
                    if i < chunk.len() {
                        let at = self
                            .offset
                            .saturating_add(u64::try_from(i).unwrap_or(u64::MAX));
                        // The delimiter is read again in `Between`.
                        self.end_number(at, sink)?;
                    }
                }
                Lex::Literal => {
                    if self.literal.get(self.literal_pos) != Some(&b) {
                        return Err(self.fail(ErrorKind::Unexpected(b), at).into());
                    }
                    self.literal_pos = self.literal_pos.saturating_add(1);
                    if self.literal_pos == self.literal.len() {
                        let token = match self.literal {
                            b"true" => Token::Bool(true),
                            b"false" => Token::Bool(false),
                            _ => Token::Null,
                        };
                        sink.token(token)?;
                        self.lex = Lex::Between;
                        self.value_done();
                    }
                    i = i.saturating_add(1);
                }
                Lex::Slash => {
                    self.lex = match b {
                        b'/' => Lex::LineComment,
                        b'*' => Lex::BlockComment,
                        _ => {
                            return Err(self
                                .fail(ErrorKind::Unexpected(b'/'), at.saturating_sub(1))
                                .into());
                        }
                    };
                    i = i.saturating_add(1);
                }
                Lex::LineComment => {
                    let rest = &chunk[i..];
                    match rest.iter().position(|&c| c == b'\n') {
                        Some(nl) => {
                            // The newline itself is counted in `Between`.
                            i = i.saturating_add(nl);
                            self.lex = Lex::Between;
                        }
                        None => i = chunk.len(),
                    }
                }
                Lex::BlockComment | Lex::BlockStar => {
                    if b == b'\n' {
                        self.newline(at);
                    }
                    self.lex = match (self.lex, b) {
                        (Lex::BlockStar, b'/') => Lex::Between,
                        (_, b'*') => Lex::BlockStar,
                        _ => Lex::BlockComment,
                    };
                    i = i.saturating_add(1);
                }
            }
        }
        Ok(())
    }

    fn newline(&mut self, at: u64) {
        self.line = self.line.saturating_add(1);
        self.line_start = at.saturating_add(1);
    }

    fn structural<S: TokenSink>(&mut self, b: u8, at: u64, sink: &mut S) -> Result<(), S::Error> {
        let object_top = self.stack.last() == Some(&true);
        match (self.expect, b) {
            (Expect::Done, _) => Err(self.fail(ErrorKind::Trailing(b), at).into()),
            (Expect::Colon, b':') => {
                self.expect = Expect::Value;
                Ok(())
            }
            (Expect::Key | Expect::KeyOrEnd, b'"') => {
                self.start_string(true);
                Ok(())
            }
            (Expect::KeyOrEnd, b'}') | (Expect::ValueOrEnd, b']') => self.close(sink),
            (Expect::CommaOrEnd, b',') => {
                self.expect = if object_top {
                    Expect::Key
                } else {
                    Expect::Value
                };
                Ok(())
            }
            (Expect::CommaOrEnd, b'}') if object_top => self.close(sink),
            (Expect::CommaOrEnd, b']') if !object_top => self.close(sink),
            (Expect::Value | Expect::ValueOrEnd, _) => self.start_value(b, at, sink),
            _ => Err(self.fail(ErrorKind::Unexpected(b), at).into()),
        }
    }

    fn start_value<S: TokenSink>(&mut self, b: u8, at: u64, sink: &mut S) -> Result<(), S::Error> {
        match b {
            b'{' | b'[' => {
                if self.stack.len() >= self.limits.max_depth {
                    return Err(self.fail(ErrorKind::TooDeep, at).into());
                }
                let object = b == b'{';
                self.stack.push(object);
                if object {
                    self.expect = Expect::KeyOrEnd;
                    sink.token(Token::StartObject)
                } else {
                    self.expect = Expect::ValueOrEnd;
                    sink.token(Token::StartArray)
                }
            }
            b'"' => {
                self.start_string(false);
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                self.scratch.clear();
                self.scratch.push(b);
                self.lex = Lex::Number;
                Ok(())
            }
            b't' | b'f' | b'n' => {
                self.literal = match b {
                    b't' => b"true",
                    b'f' => b"false",
                    _ => b"null",
                };
                self.literal_pos = 1;
                self.lex = Lex::Literal;
                Ok(())
            }
            _ => Err(self.fail(ErrorKind::Unexpected(b), at).into()),
        }
    }

    fn close<S: TokenSink>(&mut self, sink: &mut S) -> Result<(), S::Error> {
        let object = self.stack.pop() == Some(true);
        sink.token(if object {
            Token::EndObject
        } else {
            Token::EndArray
        })?;
        self.value_done();
        Ok(())
    }

    pub(super) fn value_done(&mut self) {
        self.root_seen = true;
        self.expect = if self.stack.is_empty() {
            Expect::Done
        } else {
            Expect::CommaOrEnd
        };
    }

    fn end_number<S: TokenSink>(&mut self, at: u64, sink: &mut S) -> Result<(), S::Error> {
        if !number::is_valid(&self.scratch) {
            return Err(self.fail(ErrorKind::BadNumber, at).into());
        }
        let text = std::str::from_utf8(&self.scratch).unwrap_or_default();
        sink.token(Token::Number(text))?;
        self.scratch.clear();
        self.lex = Lex::Between;
        self.value_done();
        Ok(())
    }
}

/// `BadLimits` when a string piece could not hold one character.
fn bad_limits(limits: &Limits) -> Option<JsonError> {
    (limits.string_chunk < 4).then_some(JsonError {
        kind: ErrorKind::BadLimits,
        offset: 0,
        line: 1,
        column: 1,
    })
}
