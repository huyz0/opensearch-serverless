//! The push API: a whole chunk per call, events into a sink.
//!
//! Inside a document, the common tokens (punctuation, strings without
//! escapes, numbers and literals that end inside the chunk) are handled by a
//! fast loop that never leaves this call. Anything else (an escape, a token
//! cut by the chunk's end, a document's start or end, anything malformed)
//! goes to the general state machine, which gives the exact event or error.

use crate::number::{NumState, Step};
use crate::string::{StrState, scan_plain, split_tail};
use crate::tokenizer::{Checked, Expect, Lex, Out};
use crate::{Error, Event, Text, Tokenizer};

/// Receives the events of [`Tokenizer::feed`]. Returning an error stops the
/// feed and is passed back from it.
pub trait Sink {
    /// The sink's error; a tokenizer [`Error`] converts into it.
    type Error: From<Error>;

    /// Takes one event (never `NeedMore` or `Eof`).
    fn event(&mut self, event: Event<'_>) -> Result<(), Self::Error>;
}

impl Tokenizer {
    /// Tokenizes all of `chunk`, handing each event to `sink`. Returns `true`
    /// once the input is complete (`eof` was set and nothing is missing),
    /// `false` when it needs the next chunk.
    ///
    /// The same events and errors as [`Tokenizer::next`], faster: the chunk's
    /// UTF-8 is checked once, in one SIMD pass, and common tokens skip the
    /// general state machine.
    pub fn feed<S: Sink>(
        &mut self,
        chunk: &[u8],
        eof: bool,
        sink: &mut S,
    ) -> Result<bool, S::Error> {
        let checked = self.validate(chunk);
        let start = self.offset;
        let mut pos = 0;
        loop {
            if let Some(c) = checked {
                pos = self.fast(chunk, pos, start, c, sink)?;
            }
            let (out, used) = self.step(chunk.get(pos..).unwrap_or_default(), eof)?;
            let shift = pos;
            pos = pos.saturating_add(used);
            match out {
                Out::Simple(Event::NeedMore) => return Ok(false),
                Out::Simple(Event::Eof) => return Ok(true),
                _ => sink.event(self.resolve(out, chunk, shift, checked)?)?,
            }
        }
    }

    /// Runs the fast loop from `pos`; returns where the general machine
    /// takes over. `start` is the absolute offset of `chunk[0]`.
    // ARITH: `pos` and `j` only advance past bytes read with `get`, so they
    // stay at most `chunk.len()`.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn fast<S: Sink>(
        &mut self,
        chunk: &[u8],
        mut pos: usize,
        start: u64,
        checked: Checked<'_>,
        sink: &mut S,
    ) -> Result<usize, S::Error> {
        if self.failed.get().is_some()
            || self.pending_end
            || !matches!(self.lex, Lex::None)
            || self.stack.is_empty()
        {
            return Ok(pos);
        }
        // `after_value` places a document's end relative to `base`.
        self.base = start;
        let newline_ends = self.config().framing == crate::Framing::Ndjson;
        let done = loop {
            let Some(&b) = chunk.get(pos) else { break pos };
            match (self.expect, b) {
                (_, b' ' | b'\t' | b'\r') => pos += 1,
                (_, b'\n') if !newline_ends => pos += 1,
                (Expect::Colon, b':') => {
                    self.expect = Expect::Value;
                    pos += 1;
                }
                (Expect::CommaOrEnd, b',') => {
                    let in_object = self.stack.last().copied().unwrap_or(false);
                    self.expect = if in_object {
                        Expect::Key
                    } else {
                        Expect::Value
                    };
                    pos += 1;
                }
                (Expect::CommaOrEnd | Expect::ValueOrEnd | Expect::KeyOrEnd, b'}' | b']') => {
                    let object = b == b'}';
                    let fits = match self.expect {
                        Expect::ValueOrEnd => !object,
                        Expect::KeyOrEnd => object,
                        _ => self.stack.last().copied() == Some(object),
                    };
                    if !fits {
                        break pos;
                    }
                    self.stack.pop();
                    pos += 1;
                    self.after_value(pos, false);
                    let event = if object {
                        Event::EndObject
                    } else {
                        Event::EndArray
                    };
                    self.emit_at(pos, event, sink)?;
                    if self.pending_end {
                        break pos;
                    }
                }
                (Expect::Key | Expect::KeyOrEnd, b'"') => match plain_string(chunk, pos, checked) {
                    Plain::Whole(text, end) => {
                        self.after_key();
                        pos = end;
                        let key = Event::Key(Text {
                            fragment: text,
                            first: true,
                            last: true,
                        });
                        self.emit_at(pos, key, sink)?;
                    }
                    Plain::Open(text, end) => break self.open_string(true, text, end, sink)?,
                    Plain::Other => break pos,
                },
                (Expect::Value | Expect::ValueOrEnd, _) => {
                    let Some(end) = self.fast_value(chunk, pos, checked, sink)? else {
                        break pos;
                    };
                    pos = end;
                    if !matches!(self.lex, Lex::None) {
                        break pos;
                    }
                }
                _ => break pos,
            }
        };
        self.offset = start.saturating_add(done as u64);
        Ok(done)
    }

    /// One value at `chunk[pos]`, if it is a simple one; returns the index
    /// after it, or `None` for the general machine to handle.
    // ARITH: as in `fast`; `pos + 5 <= chunk.len()` is checked by `get`.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    fn fast_value<S: Sink>(
        &mut self,
        chunk: &[u8],
        pos: usize,
        checked: Checked<'_>,
        sink: &mut S,
    ) -> Result<Option<usize>, S::Error> {
        let b = chunk.get(pos).copied().unwrap_or(0);
        let (event, end, scalar) = match b {
            b'{' | b'[' => {
                if self.stack.len() >= self.config().max_depth {
                    return Ok(None);
                }
                let object = b == b'{';
                self.stack.push(object);
                self.expect = if object {
                    Expect::KeyOrEnd
                } else {
                    Expect::ValueOrEnd
                };
                let event = if object {
                    Event::BeginObject
                } else {
                    Event::BeginArray
                };
                self.emit_at(pos + 1, event, sink)?;
                return Ok(Some(pos + 1));
            }
            b'"' => match plain_string(chunk, pos, checked) {
                Plain::Whole(text, end) => (
                    Event::String(Text {
                        fragment: text,
                        first: true,
                        last: true,
                    }),
                    end,
                    false,
                ),
                Plain::Open(text, end) => {
                    return Ok(Some(self.open_string(false, text, end, sink)?));
                }
                Plain::Other => return Ok(None),
            },
            b'-' | b'0'..=b'9' => {
                let Some(end) = number_end(chunk, pos) else {
                    return Ok(None);
                };
                if end - pos > self.config().max_number_len {
                    return Ok(None);
                }
                let Some(text) = checked.get(pos, end) else {
                    return Ok(None);
                };
                (Event::Number(text), end, true)
            }
            b't' if chunk.get(pos..pos + 4) == Some(b"true") => (Event::Bool(true), pos + 4, true),
            b'f' if chunk.get(pos..pos + 5) == Some(b"false") => {
                (Event::Bool(false), pos + 5, true)
            }
            b'n' if chunk.get(pos..pos + 4) == Some(b"null") => (Event::Null, pos + 4, true),
            _ => return Ok(None),
        };
        self.after_value(end, scalar);
        self.emit_at(end, event, sink)?;
        Ok(Some(end))
    }
}

/// What the fast scan found from a string's opening quote.
enum Plain<'a> {
    /// The whole string, escape-free, and the index after its closing quote.
    Whole(&'a str, usize),
    /// Its escape-free start, up to a backslash or (less any split
    /// character) the chunk's end, and where that start ends.
    Open(&'a str, usize),
    /// Anything else: a control character, or text outside the checked part.
    Other,
}

// ARITH: `pos + 1` and `j + 1` are indexes of bytes known to exist, and the
// split tail is at most the run's length.
#[allow(clippy::arithmetic_side_effects)]
#[inline]
fn plain_string<'a>(chunk: &[u8], pos: usize, checked: Checked<'a>) -> Plain<'a> {
    let from = pos + 1;
    let j = scan_plain(chunk, from);
    let (end, whole) = match chunk.get(j) {
        Some(b'"') => (j, true),
        Some(b'\\') => (j, false),
        Some(_) => return Plain::Other,
        None => (
            j - split_tail(&chunk[from..j]).map_or(0, |(partial, _)| partial),
            false,
        ),
    };
    match (checked.get(from, end), whole) {
        (Some(text), true) => Plain::Whole(text, j + 1),
        (Some(text), false) => Plain::Open(text, end),
        (None, _) => Plain::Other,
    }
}

impl Tokenizer {
    /// A string that the fast scan cannot finish: emits its escape-free start
    /// (if any) and leaves the rest to the string lexer at `end`.
    fn open_string<S: Sink>(
        &mut self,
        key: bool,
        text: &str,
        end: usize,
        sink: &mut S,
    ) -> Result<usize, S::Error> {
        let emitted = !text.is_empty();
        self.lex = Lex::Str(StrState::resume(key, !emitted));
        if emitted {
            let fragment = Text {
                fragment: text,
                first: true,
                last: false,
            };
            let event = if key {
                Event::Key(fragment)
            } else {
                Event::String(fragment)
            };
            self.emit_at(end, event, sink)?;
        }
        Ok(end)
    }

    /// Hands `event` to the sink, first recording that the chunk is consumed
    /// up to index `at`, so a sink error leaves `offset()` just past the
    /// event it refused.
    #[inline]
    fn emit_at<S: Sink>(
        &mut self,
        at: usize,
        event: Event<'_>,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        self.offset = self.base.saturating_add(at as u64);
        sink.event(event)
    }
}

/// The end of a complete, valid number starting at `chunk[pos]`, if a byte
/// that cannot continue it follows inside the chunk.
// ARITH: `j` only advances past a byte read with `get`.
#[allow(clippy::arithmetic_side_effects)]
#[inline]
fn number_end(chunk: &[u8], pos: usize) -> Option<usize> {
    let mut state = NumState::Start;
    let mut j = pos;
    loop {
        let &b = chunk.get(j)?;
        if b.is_ascii_digit() && matches!(state, NumState::Int | NumState::Frac | NumState::Exp) {
            j += 1;
            continue;
        }
        match state.advance(b) {
            Step::Next(next) => state = next,
            Step::End => return state.is_final().then_some(j),
            Step::Invalid => return None,
        }
        j += 1;
    }
}
