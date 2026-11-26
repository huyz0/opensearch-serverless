//! JSON text as a SMILE document: the counterpart of [`super::to_json`].
//!
//! The documents a stock repository holds are SMILE with no shared names or
//! values, so that is what is written: Jackson reads every name and string
//! as it stands, and the output needs no table to bound. Every count is
//! held to [`Limits`] and a failed call leaves the buffer as it found it.

use super::{END_STRING, Limits, WHAT};
use crate::error::{FormatError, len64};
use crate::json::{Reader, Token};

const HEADER: [u8; 4] = [0x3A, 0x29, 0x0A, 0x00];
const EMPTY: u8 = 0x20;
const NULL: u8 = 0x21;
const FALSE: u8 = 0x22;
const TRUE: u8 = 0x23;
const INT32: u8 = 0x24;
const INT64: u8 = 0x25;
const DOUBLE: u8 = 0x29;
const LONG_NAME: u8 = 0x34;
const TINY_ASCII: u8 = 0x40;
const SMALL_ASCII: u8 = 0x60;
const SHORT_ASCII_NAME: u8 = 0x80;
const TINY_UNICODE: u8 = 0x80;
const SMALL_UNICODE: u8 = 0xA0;
const SHORT_UNICODE_NAME: u8 = 0xC0;
const SMALL_INT: u8 = 0xC0;
const LONG_ASCII: u8 = 0xE0;
const LONG_UNICODE: u8 = 0xE4;
const START_ARRAY: u8 = 0xF8;
const END_ARRAY: u8 = 0xF9;
const START_OBJECT: u8 = 0xFA;
const END_OBJECT: u8 = 0xFB;

struct Writer<'o> {
    out: &'o mut Vec<u8>,
    base: usize,
    lim: Limits,
}

/// Writes the SMILE form of the JSON `input` onto `out`.
///
/// Integers that fit a long and finite doubles are written; a number past a
/// long that is not a fraction is refused, since no stock document holds
/// one. On an error `out` is left as it was.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for text that is not JSON or a number this does
/// not write, [`FormatError::TooLarge`] for a string, a nesting or an output
/// past a limit.
pub fn from_json(input: &[u8], limits: &Limits, out: &mut Vec<u8>) -> Result<(), FormatError> {
    let base = out.len();
    let mut w = Writer {
        out,
        base,
        lim: *limits,
    };
    let result = w.document(input);
    if result.is_err() {
        w.out.truncate(base);
    }
    result
}

impl Writer<'_> {
    fn document(&mut self, input: &[u8]) -> Result<(), FormatError> {
        self.put(&HEADER)?;
        let mut reader = Reader::new(input, "JSON to write as SMILE");
        let mut depth = 0usize;
        let mut any = false;
        while let Some(token) = reader.next()? {
            any = true;
            match token {
                Token::StartObject | Token::StartArray => {
                    depth = depth.saturating_add(1);
                    if depth > self.lim.depth {
                        return Err(FormatError::TooLarge {
                            what: "SMILE nesting",
                            limit: len64(self.lim.depth),
                            actual: len64(depth),
                        });
                    }
                    let open = if token == Token::StartObject {
                        START_OBJECT
                    } else {
                        START_ARRAY
                    };
                    self.put(&[open])?;
                }
                Token::EndObject => {
                    depth = depth.saturating_sub(1);
                    self.put(&[END_OBJECT])?;
                }
                Token::EndArray => {
                    depth = depth.saturating_sub(1);
                    self.put(&[END_ARRAY])?;
                }
                Token::Field(name) => self.name(name.as_bytes())?,
                Token::Str(s) => self.string(s.as_bytes())?,
                Token::Int(n) => self.int(n)?,
                Token::OtherNumber => self.double(reader.number_text())?,
                Token::Bool(b) => self.put(&[if b { TRUE } else { FALSE }])?,
                Token::Null => self.put(&[NULL])?,
            }
        }
        if any {
            Ok(())
        } else {
            Err(crate::error::corrupt(WHAT, "no JSON to write"))
        }
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        let written = self.out.len().saturating_sub(self.base);
        let total = written.saturating_add(bytes.len());
        if total > self.lim.output_bytes {
            return Err(FormatError::TooLarge {
                what: "SMILE from JSON",
                limit: len64(self.lim.output_bytes),
                actual: len64(total),
            });
        }
        self.out.extend_from_slice(bytes); // ALLOC: bounded by `output_bytes`, checked above.
        Ok(())
    }

    fn check_len(&self, bytes: &[u8]) -> Result<(), FormatError> {
        if bytes.len() > self.lim.string_bytes {
            return Err(FormatError::TooLarge {
                what: "a string or name written as SMILE",
                limit: len64(self.lim.string_bytes),
                actual: len64(bytes.len()),
            });
        }
        Ok(())
    }

    /// A string value in the shortest form the format has for it.
    fn string(&mut self, s: &[u8]) -> Result<(), FormatError> {
        self.check_len(s)?;
        if s.is_empty() {
            return self.put(&[EMPTY]);
        }
        let ascii = s.is_ascii();
        let len = s.len();
        let head = match (ascii, len) {
            (true, 1..=32) => Some(TINY_ASCII.wrapping_add(narrow(len.saturating_sub(1)))),
            (true, 33..=64) => Some(SMALL_ASCII.wrapping_add(narrow(len.saturating_sub(33)))),
            (false, 2..=33) => Some(TINY_UNICODE.wrapping_add(narrow(len.saturating_sub(2)))),
            (false, 34..=65) => Some(SMALL_UNICODE.wrapping_add(narrow(len.saturating_sub(34)))),
            _ => None,
        };
        match head {
            Some(h) => {
                self.put(&[h])?;
                self.put(s)
            }
            None => {
                self.put(&[if ascii { LONG_ASCII } else { LONG_UNICODE }])?;
                self.put(s)?;
                self.put(&[END_STRING])
            }
        }
    }

    /// An object's field name.
    fn name(&mut self, s: &[u8]) -> Result<(), FormatError> {
        self.check_len(s)?;
        if s.is_empty() {
            return self.put(&[EMPTY]);
        }
        let len = s.len();
        let head = match (s.is_ascii(), len) {
            (true, 1..=64) => Some(SHORT_ASCII_NAME.wrapping_add(narrow(len.saturating_sub(1)))),
            (false, 2..=57) => Some(SHORT_UNICODE_NAME.wrapping_add(narrow(len.saturating_sub(2)))),
            _ => None,
        };
        match head {
            Some(h) => {
                self.put(&[h])?;
                self.put(s)
            }
            None => {
                self.put(&[LONG_NAME])?;
                self.put(s)?;
                self.put(&[END_STRING])
            }
        }
    }

    fn int(&mut self, n: i64) -> Result<(), FormatError> {
        let zigzag = zigzag(n);
        if zigzag < 32 {
            return self
                .put(&[SMALL_INT.wrapping_add(narrow(usize::try_from(zigzag).unwrap_or(0)))]);
        }
        let fits32 = i32::try_from(n).is_ok();
        self.put(&[if fits32 { INT32 } else { INT64 }])?;
        self.vint(zigzag)
    }

    /// Seven bits at a time, most significant first, the last byte carrying
    /// six bits and the high bit.
    fn vint(&mut self, mut v: u64) -> Result<(), FormatError> {
        let mut tail = [0u8; 10];
        let mut at = tail.len();
        at = at.saturating_sub(1);
        if let Some(b) = tail.get_mut(at) {
            *b = 0x80 | u8::try_from(v & 0x3F).unwrap_or(0);
        }
        v >>= 6;
        while v > 0 {
            at = at.saturating_sub(1);
            if let Some(b) = tail.get_mut(at) {
                *b = u8::try_from(v & 0x7F).unwrap_or(0);
            }
            v >>= 7;
        }
        self.put(tail.get(at..).unwrap_or_default())
    }

    fn double(&mut self, text: &str) -> Result<(), FormatError> {
        let fraction = text.bytes().any(|b| matches!(b, b'.' | b'e' | b'E'));
        let value = text
            .parse::<f64>()
            .ok()
            .filter(|v| fraction && v.is_finite());
        let Some(value) = value else {
            return Err(crate::error::corrupt(
                WHAT,
                "a number that is neither a long nor a finite double",
            ));
        };
        let bits = value.to_bits();
        let mut bytes = [0u8; 11];
        bytes[0] = DOUBLE;
        bytes[1] = u8::try_from(bits >> 63).unwrap_or(0);
        for i in 0..9u32 {
            let shift = 56u32.saturating_sub(i.saturating_mul(7));
            let at = usize::try_from(i).unwrap_or(0).saturating_add(2);
            if let Some(b) = bytes.get_mut(at) {
                *b = u8::try_from((bits >> shift) & 0x7F).unwrap_or(0);
            }
        }
        self.put(&bytes)
    }
}

fn zigzag(n: i64) -> u64 {
    // The sign bit smeared over the value, then the value doubled.
    u64::from_ne_bytes(((n << 1) ^ (n >> 63)).to_ne_bytes())
}

/// A small count as the byte it is added into; the caller's match arms
/// keep it under 32.
fn narrow(n: usize) -> u8 {
    u8::try_from(n).unwrap_or(0)
}
