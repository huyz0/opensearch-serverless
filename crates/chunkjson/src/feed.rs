//! The push API: a whole chunk per call, events into a sink.
//!
//! Inside a document, the common tokens (punctuation, strings without
//! escapes, numbers and literals that end inside the chunk) are handled by a
//! fast loop that never leaves this call. Anything else (an escape, a token
//! cut by the chunk's end, a document's start or end, anything malformed)
//! goes to the general state machine, which gives the exact event or error.

use crate::number::number_end;
use crate::string::{StrState, scan_plain, split_tail};
use crate::tokenizer::{Checked, Expect, Lex, Out};
use crate::{Error, Event, Position, Text, Tokenizer};

/// Receives the events of [`Tokenizer::feed`]. Returning an error stops the
/// feed and is passed back from it.
pub trait Sink {
    /// The sink's error; a tokenizer [`Error`] converts into it.
    type Error: From<Error>;

    /// Takes one event (never `NeedMore` or `Eof`).
    fn event(&mut self, event: Event<'_>) -> Result<(), Self::Error>;

    /// Takes one event with where its token starts (see
    /// [`Tokenizer::event_start`]). The tokenizer calls this; by default it
    /// drops the position and calls [`Sink::event`], which a sink must still
    /// implement even when it overrides this.
    #[inline]
    fn event_at(&mut self, event: Event<'_>, start: Position) -> Result<(), Self::Error> {
        let _ = start;
        self.event(event)
    }
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
                let next = self.fast(chunk, pos, start, c, sink)?;
                debug_assert!(next >= pos, "the fast loop never moves back");
                pos = next;
            }
            let (out, used) = self.step(chunk.get(pos..).unwrap_or_default(), eof)?;
            let shift = pos;
            pos = pos.saturating_add(used);
            match out {
                Out::Simple(Event::NeedMore) => return Ok(false),
                Out::Simple(Event::Eof) => return Ok(true),
                _ => {
                    let start = self.position();
                    sink.event_at(self.resolve(out, chunk, shift, checked)?, start)?;
                }
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
        // Every pass that does not stop advances `pos`, so the loop is
        // bounded by the chunk length.
        let done = 'scan: {
            for _ in 0..chunk.len() {
                let Some(&b) = chunk.get(pos) else {
                    break 'scan pos;
                };
                match (self.expect, b) {
                    (_, b' ' | b'\t' | b'\r') => pos += 1,
                    (_, b'\n') if !newline_ends => {
                        pos += 1;
                        self.newline(pos);
                    }
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
                        self.start = start + pos as u64;
                        let object = b == b'}';
                        // `ValueOrEnd` and `KeyOrEnd` follow a push, so the top
                        // of the stack already says which closer fits.
                        if self.stack.last().copied() != Some(object) {
                            break 'scan pos;
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
                            break 'scan pos;
                        }
                    }
                    (Expect::Key | Expect::KeyOrEnd, b'"') => {
                        self.start = start + pos as u64;
                        match plain_string(chunk, pos, checked) {
                            Plain::Whole(text, end) => {
                                self.after_key();
                                pos = end;
                                let key = Event::Key(Text {
                                    fragment: text,
                                    first: true,
                                    last: true,
                                });
                                self.emit_at(pos, key, sink)?;
                                // a colon right after a key is the common case
                                if chunk.get(pos) == Some(&b':') {
                                    self.expect = Expect::Value;
                                    pos += 1;
                                }
                            }
                            Plain::Open(text, end) => {
                                break 'scan self.open_string(true, text, end, sink)?;
                            }
                            Plain::Other => break 'scan pos,
                        }
                    }
                    (Expect::Value | Expect::ValueOrEnd, _) => {
                        self.start = start + pos as u64;
                        let Some(end) = self.fast_value(chunk, pos, checked, sink)? else {
                            break 'scan pos;
                        };
                        pos = end;
                        if !matches!(self.lex, Lex::None) {
                            break 'scan pos;
                        }
                    }
                    _ => break 'scan pos,
                }
            }
            pos
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
#[inline(always)]
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
        sink.event_at(event, self.position())
    }
}

#[cfg(test)]
// ARITH: test arithmetic on small literal offsets and counts.
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::Config;

    struct Count(usize);

    impl Sink for Count {
        type Error = Error;
        fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
            assert!(!matches!(event, Event::NeedMore | Event::Eof));
            self.0 += 1;
            Ok(())
        }
    }

    /// The fast loop is a speed path: falling back to the general machine
    /// gives the same events, so these tests check that it is taken.
    #[test]
    fn the_fast_loop_takes_simple_tokens_to_the_chunk_end() {
        let mut tok = Tokenizer::new(Config::default());
        let mut sink = Count(0);
        assert!(!tok.feed(b"[", false, &mut sink).unwrap());
        let chunk = br#"1, -2.5e3 ,"a",true,false,null,{"k":"v","n":[]},"#;
        let checked = tok.validate(chunk).expect("valid UTF-8");
        let start = tok.offset();
        let end = tok.fast(chunk, 0, start, checked, &mut sink).unwrap();
        assert_eq!(end, chunk.len());
        assert_eq!(tok.offset(), start + chunk.len() as u64);
        assert_eq!(
            sink.0,
            1 + 6 + 7,
            "BeginArray, six scalars, then the object's seven events"
        );
    }

    #[test]
    fn a_string_cut_by_the_chunk_end_keeps_its_complete_characters() {
        let chunk = b"\"ab\xc3";
        let checked = Tokenizer::new(Config::default()).validate(chunk).unwrap();
        match plain_string(chunk, 0, checked) {
            Plain::Open(text, end) => assert_eq!((text, end), ("ab", 3)),
            _ => panic!("expected an open string"),
        }
        let chunk = b"\"x\\n\"";
        let checked = Tokenizer::new(Config::default()).validate(chunk).unwrap();
        assert!(matches!(
            plain_string(chunk, 0, checked),
            Plain::Open("x", 2)
        ));
        assert!(matches!(
            plain_string(b"\"ok\"", 0, checked_of(b"\"ok\"")),
            Plain::Whole("ok", 4)
        ));
    }

    struct Refuse;

    impl Sink for Refuse {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            Err(Error::new(crate::ErrorKind::UnexpectedEof, 0))
        }
    }

    /// Runs the fast loop on `chunk` after `prefix` was fed; returns where it
    /// stopped.
    fn fast_after(config: Config, prefix: &[u8], chunk: &[u8]) -> usize {
        let mut tok = Tokenizer::new(config);
        let mut sink = Count(0);
        let _ = tok.feed(prefix, false, &mut sink);
        let checked = tok.validate(chunk).expect("valid UTF-8");
        let start = tok.offset();
        tok.fast(chunk, 0, start, checked, &mut sink).unwrap()
    }

    #[test]
    fn newlines_inside_a_document_stay_in_the_fast_loop_unless_they_frame() {
        let chunk = b"1,\n2,";
        assert_eq!(fast_after(Config::default(), b"[", chunk), chunk.len());
        let ndjson = Config::with_framing(crate::Framing::Ndjson);
        assert_eq!(
            fast_after(ndjson, b"[", chunk),
            2,
            "NDJSON stops at the newline"
        );
    }

    #[test]
    fn a_number_at_the_length_limit_stays_in_the_fast_loop() {
        let config = Config {
            max_number_len: 3,
            ..Config::default()
        };
        assert_eq!(fast_after(config, b"[", b" 123,"), 5);
        assert_eq!(
            fast_after(config, b"[", b" 1234,"),
            1,
            "too long: general machine"
        );
    }

    #[test]
    fn a_failed_tokenizer_skips_the_fast_loop() {
        assert_eq!(fast_after(Config::default(), b"[1}", b", 2,"), 0);
    }

    #[test]
    fn a_refused_open_bracket_leaves_the_offset_after_it() {
        let mut tok = Tokenizer::new(Config::default());
        assert!(!tok.feed(b"[", false, &mut Count(0)).unwrap());
        let chunk = b"  [";
        let checked = tok.validate(chunk).unwrap();
        let start = tok.offset();
        assert!(tok.fast(chunk, 0, start, checked, &mut Refuse).is_err());
        assert_eq!(tok.offset(), start + 3);
    }

    fn checked_of(chunk: &[u8]) -> Checked<'_> {
        Tokenizer::new(Config::default()).validate(chunk).unwrap()
    }

    #[test]
    fn validation_skips_only_the_bytes_completing_a_split_character() {
        let mut tok = Tokenizer::new(Config::default());
        let mut sink = Count(0);
        assert!(!tok.feed(b"[\"\xc3", false, &mut sink).unwrap());
        let next = b"\xa9b\"";
        let checked = tok.validate(next).expect("rest is UTF-8");
        assert_eq!(checked.get(1, 2), Some("b"));
        assert_eq!(
            checked.get(0, 2),
            None,
            "the continuation byte is the lexer's"
        );
        // once the split character is complete, nothing is skipped
        assert!(!tok.feed(next, false, &mut sink).unwrap());
        let checked = tok.validate(b"abc").unwrap();
        assert_eq!(checked.get(0, 3), Some("abc"));
    }
}
