//! Turning a step's lifetime-free output into an event that borrows its
//! text, and the one-pass UTF-8 check that lets spans skip their own.

use crate::tokenizer::{Out, Src, Tokenizer};
use crate::{Error, ErrorKind, Event, Text};

/// `text` is `input[from..from + text.len()]`, already validated as UTF-8.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Checked<'a> {
    text: &'a str,
    from: usize,
}

impl<'a> Checked<'a> {
    /// `input[from..to]` as text, if it lies inside the validated part on
    /// character boundaries.
    #[inline]
    pub(crate) fn get(self, from: usize, to: usize) -> Option<&'a str> {
        self.text
            .get(from.checked_sub(self.from)?..to.checked_sub(self.from)?)
    }
}

impl Tokenizer {
    /// Borrows the text of an event produced by [`Tokenizer::step`] over
    /// `input[shift..]`. `checked`, when given, is a part of `input` already
    /// validated as UTF-8, so spans inside it need no second check.
    #[inline]
    pub(crate) fn resolve<'a>(
        &'a self,
        out: Out,
        input: &'a [u8],
        shift: usize,
        checked: Option<Checked<'a>>,
    ) -> Result<Event<'a>, Error> {
        match out {
            Out::Simple(event) => Ok(event),
            Out::Number(src) => Ok(Event::Number(self.text(src, input, shift, checked)?)),
            Out::Text {
                key,
                src,
                first,
                last,
            } => {
                let text = Text {
                    fragment: self.text(src, input, shift, checked)?,
                    first,
                    last,
                };
                Ok(if key {
                    Event::Key(text)
                } else {
                    Event::String(text)
                })
            }
        }
    }

    #[inline]
    fn text<'a>(
        &'a self,
        src: Src,
        input: &'a [u8],
        shift: usize,
        checked: Option<Checked<'a>>,
    ) -> Result<&'a str, Error> {
        let (bytes, at) = match src {
            Src::Input(start, end) => {
                let (from, to) = (start.saturating_add(shift), end.saturating_add(shift));
                if let Some(text) = checked.and_then(|c| c.get(from, to)) {
                    return Ok(text);
                }
                (input.get(from..to).unwrap_or_default(), start)
            }
            Src::Char => (self.char_buf.get(..self.char_len).unwrap_or_default(), 0),
            Src::NumBuf => (self.numbuf.as_slice(), 0),
        };
        if let Ok(text) = simdutf8::basic::from_utf8(bytes) {
            return Ok(text);
        }
        // The slow path, only to locate the fault.
        std::str::from_utf8(bytes).map_err(|e| {
            let err = self.err(ErrorKind::InvalidUtf8, at.saturating_add(e.valid_up_to()));
            self.failed.set(Some(err));
            err
        })
    }

    /// The part of `chunk` that is UTF-8, found with one SIMD pass, so every
    /// string span inside it is borrowed without its own check. Leaves out
    /// the bytes completing a character split from the previous chunk and a
    /// character split at the chunk's end; the string lexer checks those.
    pub(crate) fn validate<'a>(&self, chunk: &'a [u8]) -> Option<Checked<'a>> {
        let skip = if self.utf8_len > 0 {
            self.utf8_need
                .saturating_sub(self.utf8_len)
                .min(chunk.len())
        } else {
            0
        };
        let body = chunk.get(skip..)?;
        let tail = crate::string::split_tail(body).map_or(0, |(partial, _)| partial);
        let body = body.get(..body.len().saturating_sub(tail))?;
        let text = match simdutf8::compat::from_utf8(body) {
            Ok(text) => text,
            Err(e) => std::str::from_utf8(body.get(..e.valid_up_to())?).ok()?,
        };
        Some(Checked { text, from: skip })
    }
}
