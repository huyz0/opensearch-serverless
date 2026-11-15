//! The sans-IO tokenizer: structure, numbers and literals. Strings are in
//! `string.rs`, which shares this state.

use std::cell::Cell;

use crate::number::NumState;
pub(crate) use crate::resolve::Checked;
use crate::string::StrState;
use crate::{Config, Error, ErrorKind, Event, Framing};

/// What the grammar allows next, outside any token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expect {
    /// Before the first document.
    DocStart,
    /// A value (after `:` or `,` in an array).
    Value,
    /// A value or `]` (just after `[`).
    ValueOrEnd,
    /// A key or `}` (just after `{`).
    KeyOrEnd,
    /// A key (after `,` in an object).
    Key,
    /// `:` after a key.
    Colon,
    /// `,` or the container's closing bracket.
    CommaOrEnd,
    /// After a complete top-level document.
    AfterDoc,
}

/// The token being lexed, if any. A token may span any number of calls.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Lex {
    None,
    Str(StrState),
    Num(NumState),
    Lit { word: &'static [u8], matched: usize },
}

/// Where an event's text lives: in the caller's slice, or in the tokenizer.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Src {
    /// `input[start..end]` of the slice passed to the step.
    Input(usize, usize),
    /// The decoded character in `char_buf`.
    Char,
    /// The number buffer.
    NumBuf,
}

/// An event before its text is borrowed, so it carries no lifetime and a
/// caller can refill its buffer between a `NeedMore` and the next step.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Out {
    Simple(Event<'static>),
    Text {
        key: bool,
        src: Src,
        first: bool,
        last: bool,
    },
    Number(Src),
}

/// A streaming JSON tokenizer. Feed it byte slices with [`Tokenizer::next`];
/// it returns one event per call and remembers everything it needs between
/// calls, so a slice may end anywhere, even inside a token or a UTF-8 character.
///
/// Memory: the nesting stack (`max_depth` entries) and the number buffer
/// (`max_number_len` bytes) are allocated once, at construction. Nothing else
/// is allocated per call.
#[derive(Debug)]
pub struct Tokenizer {
    pub(crate) config: Config,
    /// One entry per open container: `true` for an object.
    pub(crate) stack: Vec<bool>,
    pub(crate) expect: Expect,
    pub(crate) lex: Lex,
    /// A top-level value just completed; `EndDocument` is emitted next.
    pub(crate) pending_end: bool,
    /// Inside a top-level document (for NDJSON's line rule).
    in_doc: bool,
    /// NDJSON: a document ended on the current line.
    line_open: bool,
    /// The last document was a number or literal.
    after_scalar: bool,
    doc_start: u64,
    doc_end: u64,
    /// Bytes consumed by every previous call.
    pub(crate) offset: u64,
    /// `offset` when the current call began: errors add their index to it.
    pub(crate) base: u64,
    pub(crate) numbuf: Vec<u8>,
    /// The bytes of a UTF-8 character split across slices.
    pub(crate) utf8: [u8; 4],
    pub(crate) utf8_len: usize,
    pub(crate) utf8_need: usize,
    /// Absolute offset of the split character's first byte.
    pub(crate) utf8_at: u64,
    /// The UTF-8 encoding of the last decoded escape or split character.
    pub(crate) char_buf: [u8; 4],
    pub(crate) char_len: usize,
    pub(crate) failed: Cell<Option<Error>>,
}

impl Tokenizer {
    /// A tokenizer with these limits and framing.
    pub fn new(config: Config) -> Self {
        Self {
            // ALLOC: once per tokenizer, at most `max_depth` entries.
            stack: Vec::with_capacity(config.max_depth),
            // ALLOC: once per tokenizer, at most `max_number_len` bytes.
            numbuf: Vec::with_capacity(config.max_number_len),
            config,
            expect: Expect::DocStart,
            lex: Lex::None,
            pending_end: false,
            in_doc: false,
            line_open: false,
            after_scalar: false,
            doc_start: 0,
            doc_end: 0,
            offset: 0,
            base: 0,
            utf8: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
            utf8_at: 0,
            char_buf: [0; 4],
            char_len: 0,
            failed: Cell::new(None),
        }
    }

    /// The configuration this tokenizer was built with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Bytes consumed so far, over every call. After an error from the
    /// tokenizer it may include whitespace before the fault; after a sink
    /// error in [`Tokenizer::feed`] it is just past the refused event.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Containers currently open.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Advances over `input` and returns the next event and how many bytes of
    /// `input` it consumed. The caller passes the unconsumed rest next time.
    ///
    /// `NeedMore` means all of `input` was consumed; call again with the next
    /// bytes. Pass `eof = true` once no more bytes will come: then a number at
    /// the very end completes, and the input either ends cleanly with `Eof` or
    /// fails with `UnexpectedEof`. An event may consume zero bytes.
    ///
    /// After an error, every later call returns the same error.
    #[inline]
    pub fn next<'a>(&'a mut self, input: &'a [u8], eof: bool) -> Result<(Event<'a>, usize), Error> {
        let (out, used) = self.step(input, eof)?;
        Ok((self.resolve(out, input, 0, None)?, used))
    }

    /// The lifetime-free half of [`Tokenizer::next`].
    #[inline]
    pub(crate) fn step(&mut self, input: &[u8], eof: bool) -> Result<(Out, usize), Error> {
        if let Some(e) = self.failed.get() {
            return Err(e);
        }
        self.base = self.offset;
        match self.step_inner(input, eof) {
            Ok((out, used)) => {
                self.offset = self.offset.saturating_add(used as u64);
                Ok((out, used))
            }
            Err(e) => {
                self.failed.set(Some(e));
                Err(e)
            }
        }
    }

    /// An error at index `i` of the current call's input.
    pub(crate) fn err(&self, kind: ErrorKind, i: usize) -> Error {
        Error::new(kind, self.base.saturating_add(i as u64))
    }

    // ARITH: `i` only advances past a byte read with `input.get(i)`, so it
    // stays at most `input.len()` and `i + 1` cannot overflow.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn step_inner(&mut self, input: &[u8], eof: bool) -> Result<(Out, usize), Error> {
        if self.pending_end {
            self.pending_end = false;
            self.in_doc = false;
            let end = Event::EndDocument {
                start: self.doc_start,
                end: self.doc_end,
            };
            return Ok((Out::Simple(end), 0));
        }
        let mut i = 0;
        loop {
            match self.lex {
                Lex::Str(_) => return self.lex_string(input, i, eof),
                Lex::Num(_) => return self.lex_number(input, i, eof),
                Lex::Lit { .. } => return self.lex_literal(input, i, eof),
                Lex::None => {}
            }
            let Some(&b) = input.get(i) else {
                return self.at_end(i, eof);
            };
            match b {
                b' ' | b'\t' | b'\r' => {
                    self.after_scalar = false;
                    i += 1;
                }
                b'\n' => {
                    self.after_scalar = false;
                    if self.in_doc && self.config.framing == Framing::Ndjson {
                        return Err(self.err(ErrorKind::NewlineInDocument, i));
                    }
                    self.line_open = false;
                    i += 1;
                }
                _ => {
                    if let Some(out) = self.structural(b, &mut i)? {
                        return Ok((out, i));
                    }
                }
            }
        }
    }

    fn at_end(&mut self, i: usize, eof: bool) -> Result<(Out, usize), Error> {
        if !eof {
            return Ok((Out::Simple(Event::NeedMore), i));
        }
        match self.expect {
            Expect::DocStart if self.config.framing == Framing::Single => {
                Err(self.err(ErrorKind::UnexpectedEof, i))
            }
            Expect::DocStart | Expect::AfterDoc => Ok((Out::Simple(Event::Eof), i)),
            _ => Err(self.err(ErrorKind::UnexpectedEof, i)),
        }
    }

    /// Handles a non-whitespace byte outside any token. Returns an event, or
    /// `None` when a token began and lexing continues in the same call.
    // ARITH: `*i` indexes the byte `b` just read, so `*i + 1 <= input.len()`.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn structural(&mut self, b: u8, i: &mut usize) -> Result<Option<Out>, Error> {
        let unexpected = |t: &Self| Err(t.err(ErrorKind::UnexpectedByte(b), *i));
        match self.expect {
            Expect::DocStart | Expect::AfterDoc | Expect::Value | Expect::ValueOrEnd => {
                if self.expect == Expect::ValueOrEnd && b == b']' {
                    self.stack.pop();
                    *i += 1;
                    self.after_value(*i, false);
                    return Ok(Some(Out::Simple(Event::EndArray)));
                }
                if self.expect == Expect::AfterDoc {
                    let refused = match self.config.framing {
                        Framing::Single => true,
                        Framing::Ndjson => self.line_open,
                        // As serde_json: `1true` or `1-2` is one bad
                        // document, not two good ones.
                        Framing::Concatenated => {
                            self.after_scalar
                                && !matches!(b, b'[' | b'{' | b'"' | b']' | b'}' | b',' | b':')
                        }
                    };
                    if refused {
                        return Err(self.err(ErrorKind::TrailingData, *i));
                    }
                }
                if matches!(self.expect, Expect::DocStart | Expect::AfterDoc) {
                    self.in_doc = true;
                    self.doc_start = self.base.saturating_add(*i as u64);
                }
                self.begin_value(b, i)
            }
            Expect::KeyOrEnd | Expect::Key => match b {
                b'"' => {
                    self.lex = Lex::Str(StrState::new(true));
                    *i += 1;
                    Ok(None)
                }
                b'}' if self.expect == Expect::KeyOrEnd => {
                    self.stack.pop();
                    *i += 1;
                    self.after_value(*i, false);
                    Ok(Some(Out::Simple(Event::EndObject)))
                }
                _ => unexpected(self),
            },
            Expect::Colon if b == b':' => {
                self.expect = Expect::Value;
                *i += 1;
                Ok(None)
            }
            Expect::CommaOrEnd => {
                let in_object = self.stack.last().copied().unwrap_or(false);
                match (b, in_object) {
                    (b',', true) => self.expect = Expect::Key,
                    (b',', false) => self.expect = Expect::Value,
                    (b'}', true) | (b']', false) => {
                        self.stack.pop();
                        *i += 1;
                        self.after_value(*i, false);
                        let event = if in_object {
                            Event::EndObject
                        } else {
                            Event::EndArray
                        };
                        return Ok(Some(Out::Simple(event)));
                    }
                    _ => return unexpected(self),
                }
                *i += 1;
                Ok(None)
            }
            Expect::Colon => unexpected(self),
        }
    }

    // ARITH: as in `structural`.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn begin_value(&mut self, b: u8, i: &mut usize) -> Result<Option<Out>, Error> {
        match b {
            b'{' | b'[' => {
                if self.stack.len() >= self.config.max_depth {
                    return Err(self.err(ErrorKind::DepthExceeded, *i));
                }
                let object = b == b'{';
                self.stack.push(object);
                *i += 1;
                if object {
                    self.expect = Expect::KeyOrEnd;
                    Ok(Some(Out::Simple(Event::BeginObject)))
                } else {
                    self.expect = Expect::ValueOrEnd;
                    Ok(Some(Out::Simple(Event::BeginArray)))
                }
            }
            b'"' => {
                self.lex = Lex::Str(StrState::new(false));
                *i += 1;
                Ok(None)
            }
            b'-' | b'0'..=b'9' => {
                self.numbuf.clear();
                self.lex = Lex::Num(NumState::Start);
                Ok(None)
            }
            b't' => self.begin_literal(b"true"),
            b'f' => self.begin_literal(b"false"),
            b'n' => self.begin_literal(b"null"),
            _ => Err(self.err(ErrorKind::UnexpectedByte(b), *i)),
        }
    }

    fn begin_literal(&mut self, word: &'static [u8]) -> Result<Option<Out>, Error> {
        self.lex = Lex::Lit { word, matched: 0 };
        Ok(None)
    }

    /// A value ended at index `i` of the current input; `scalar` for a
    /// number or literal, whose end only a delimiter can mark.
    #[inline]
    pub(crate) fn after_value(&mut self, i: usize, scalar: bool) {
        if self.stack.is_empty() {
            self.after_scalar = scalar;
            self.expect = Expect::AfterDoc;
            self.pending_end = true;
            self.line_open = true;
            self.doc_end = self.base.saturating_add(i as u64);
        } else {
            self.expect = Expect::CommaOrEnd;
        }
    }

    /// A key ended: a `:` comes next.
    #[inline]
    pub(crate) fn after_key(&mut self) {
        self.expect = Expect::Colon;
    }

    // ARITH: `j` only advances past a byte read with `input.get(j)`, and
    // `matched` stays below `word.len()` (at most 5) before each increment.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn lex_literal(
        &mut self,
        input: &[u8],
        start: usize,
        eof: bool,
    ) -> Result<(Out, usize), Error> {
        let Lex::Lit { word, mut matched } = self.lex else {
            return Err(self.err(ErrorKind::UnexpectedEof, start));
        };
        let mut j = start;
        while let Some(&want) = word.get(matched) {
            let Some(&b) = input.get(j) else {
                if eof {
                    return Err(self.err(ErrorKind::UnexpectedEof, j));
                }
                self.lex = Lex::Lit { word, matched };
                return Ok((Out::Simple(Event::NeedMore), j));
            };
            if b != want {
                return Err(self.err(ErrorKind::UnexpectedByte(b), j));
            }
            matched += 1;
            j += 1;
        }
        self.lex = Lex::None;
        let event = match word {
            b"true" => Event::Bool(true),
            b"false" => Event::Bool(false),
            _ => Event::Null,
        };
        self.after_value(j, true);
        Ok((Out::Simple(event), j))
    }
}
