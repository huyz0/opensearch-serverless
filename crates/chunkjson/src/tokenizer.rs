//! The sans-IO tokenizer: structure, numbers and literals. Strings are in
//! `string.rs`, which shares this state.

use std::cell::Cell;

use crate::comment::Comment;
use crate::number::NumState;
pub(crate) use crate::resolve::Checked;
use crate::string::StrState;
use crate::{Config, Error, ErrorKind, Event, Framing, Position};

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
    Comment(Comment),
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
    pub(crate) in_doc: bool,
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
    /// The current 1-based line, counted at line breaks between tokens.
    pub(crate) line: u64,
    /// Absolute offset of the current line's first byte.
    pub(crate) line_start: u64,
    /// Absolute offset where the current token started.
    pub(crate) start: u64,
    /// Where the current document started.
    pub(crate) doc_pos: Position,
    /// The last event was `EndDocument`, whose start is `doc_pos`.
    pub(crate) doc_ended: bool,
    /// Absolute offset of the `/` that opened the current comment.
    pub(crate) comment_at: u64,
}

impl Tokenizer {
    /// A tokenizer with these limits and framing.
    pub fn new(config: Config) -> Self {
        Self::with_buffers(
            config,
            // ALLOC: once per tokenizer, at most `max_depth` entries.
            Vec::with_capacity(config.max_depth),
            // ALLOC: once per tokenizer, at most `max_number_len` bytes.
            Vec::with_capacity(config.max_number_len),
        )
    }

    /// Returns to the state after [`Tokenizer::new`], keeping the
    /// allocations, so a caller can tokenize one document after another
    /// without allocating.
    pub fn reset(&mut self) {
        let mut stack = std::mem::take(&mut self.stack);
        stack.clear();
        let mut numbuf = std::mem::take(&mut self.numbuf);
        numbuf.clear();
        *self = Self::with_buffers(self.config, stack, numbuf);
    }

    /// The one place every field gets its starting value.
    fn with_buffers(config: Config, stack: Vec<bool>, numbuf: Vec<u8>) -> Self {
        Self {
            stack,
            numbuf,
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
            line: 1,
            line_start: 0,
            start: 0,
            doc_pos: Position {
                offset: 0,
                line: 1,
                column: 1,
            },
            doc_ended: false,
            comment_at: 0,
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

    /// As [`Tokenizer::next`], with where the event's token starts (see
    /// [`Tokenizer::event_start`]), since the event borrows the tokenizer.
    #[inline]
    pub fn next_at<'a>(
        &'a mut self,
        input: &'a [u8],
        eof: bool,
    ) -> Result<(Event<'a>, usize, Position), Error> {
        let (out, used) = self.step(input, eof)?;
        let start = self.position();
        Ok((self.resolve(out, input, 0, None)?, used, start))
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
        self.err_abs(kind, self.base.saturating_add(i as u64))
    }

    // ARITH: `i` only advances past a byte read with `input.get(i)`, so it
    // stays at most `input.len()` and `i + 1` cannot overflow.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn step_inner(&mut self, input: &[u8], eof: bool) -> Result<(Out, usize), Error> {
        if self.pending_end {
            self.pending_end = false;
            self.in_doc = false;
            self.doc_ended = true;
            let end = Event::EndDocument {
                start: self.doc_start,
                end: self.doc_end,
            };
            return Ok((Out::Simple(end), 0));
        }
        let mut i = 0;
        // Every pass consumes a byte, starts or ends a token, or returns.
        // Debug builds catch a pass that leaves both the index and the lexer
        // state as the pass before left them; a longer cycle is not caught.
        #[cfg(debug_assertions)]
        let mut last = None;
        loop {
            #[cfg(debug_assertions)]
            {
                let now = Some((i, std::mem::discriminant(&self.lex)));
                debug_assert!(now != last, "the tokenizer made no progress");
                last = now;
            }
            match self.lex {
                Lex::Str(_) => return self.lex_string(input, i, eof),
                Lex::Num(_) => return self.lex_number(input, i, eof),
                Lex::Lit { .. } => return self.lex_literal(input, i, eof),
                Lex::Comment(state) => {
                    if let Some(out) = self.lex_comment(state, input, &mut i, eof)? {
                        return Ok(out);
                    }
                    continue;
                }
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
                    self.newline(i);
                }
                b'/' if self.config.comments => {
                    self.after_scalar = false;
                    self.lex = Lex::Comment(Comment::Slash);
                    self.comment_at = self.base.saturating_add(i as u64);
                    i += 1;
                }
                _ => {
                    self.start = self.base.saturating_add(i as u64);
                    self.doc_ended = false;
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
            Expect::DocStart
                if self.config.framing == Framing::Single && !self.config.allow_empty =>
            {
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
                    self.doc_pos = self.position();
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
}
