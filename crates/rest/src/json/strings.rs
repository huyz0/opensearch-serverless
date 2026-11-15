//! Strings: escapes, surrogate pairs, UTF-8 checks and handing a long value
//! on in pieces.

use super::lexer::{Expect, Lex, Tokenizer};
use super::{ErrorKind, Token, TokenSink};

impl Tokenizer {
    pub(super) fn start_string(&mut self, key: bool) {
        self.scratch.clear();
        self.in_key = key;
        self.string_len = 0;
        self.high = None;
        self.lex = Lex::Str;
    }

    pub(super) fn push_str<S: TokenSink>(
        &mut self,
        mut bytes: &[u8],
        at: u64,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        let n = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.string_len = self.string_len.saturating_add(n);
        if self.in_key {
            if self.scratch.len().saturating_add(bytes.len()) > self.limits.max_key_bytes {
                return Err(self.fail(ErrorKind::KeyTooLong, at).into());
            }
            self.scratch.extend_from_slice(bytes);
            return Ok(());
        }
        if self.string_len > self.limits.max_string_bytes {
            return Err(self.fail(ErrorKind::StringTooLong, at).into());
        }
        let piece = self.limits.string_chunk;
        while !bytes.is_empty() {
            let room = piece.saturating_sub(self.scratch.len());
            let take = room.min(bytes.len());
            self.scratch.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.scratch.len() >= piece {
                self.flush_piece(at, sink)?;
            }
        }
        Ok(())
    }

    /// Hands on the scratch's complete characters as a non-final piece,
    /// keeping an incomplete trailing character for the next one.
    pub(super) fn flush_piece<S: TokenSink>(
        &mut self,
        at: u64,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        let valid = match std::str::from_utf8(&self.scratch) {
            Ok(_) => self.scratch.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => return Err(self.fail(ErrorKind::InvalidUtf8, at).into()),
        };
        if valid > 0 {
            let text = std::str::from_utf8(&self.scratch[..valid]).unwrap_or_default();
            sink.token(Token::Str { text, last: false })?;
        }
        let len = self.scratch.len();
        self.scratch.copy_within(valid..len, 0);
        self.scratch.truncate(len.saturating_sub(valid));
        Ok(())
    }

    pub(super) fn end_string<S: TokenSink>(
        &mut self,
        at: u64,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        let Ok(text) = std::str::from_utf8(&self.scratch) else {
            return Err(self.fail(ErrorKind::InvalidUtf8, at).into());
        };
        self.lex = Lex::Between;
        if self.in_key {
            sink.token(Token::Key(text))?;
            self.expect = Expect::Colon;
        } else {
            sink.token(Token::Str { text, last: true })?;
            self.value_done();
        }
        self.scratch.clear();
        Ok(())
    }

    pub(super) fn escape<S: TokenSink>(
        &mut self,
        b: u8,
        at: u64,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        if self.high.is_some() && b != b'u' {
            return Err(self.fail(ErrorKind::LoneSurrogate, at).into());
        }
        let decoded = match b {
            b'"' => b'"',
            b'\\' => b'\\',
            b'/' => b'/',
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                self.unicode = 0;
                self.unicode_digits = 0;
                self.lex = Lex::Unicode;
                return Ok(());
            }
            _ => return Err(self.fail(ErrorKind::BadEscape, at).into()),
        };
        self.lex = Lex::Str;
        self.push_str(&[decoded], at, sink)
    }

    pub(super) fn unicode_digit<S: TokenSink>(
        &mut self,
        b: u8,
        at: u64,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        let Some(d) = char::from(b).to_digit(16) else {
            return Err(self.fail(ErrorKind::BadEscape, at).into());
        };
        self.unicode = self.unicode.wrapping_shl(4) | d;
        self.unicode_digits = self.unicode_digits.saturating_add(1);
        if self.unicode_digits < 4 {
            return Ok(());
        }
        self.lex = Lex::Str;
        let code = self.unicode;
        let scalar = match (self.high.take(), code) {
            (Some(high), 0xDC00..=0xDFFF) => combine(high, code),
            (Some(_), _) | (None, 0xDC00..=0xDFFF) => {
                return Err(self.fail(ErrorKind::LoneSurrogate, at).into());
            }
            (None, 0xD800..=0xDBFF) => {
                self.high = Some(code);
                return Ok(());
            }
            (None, c) => c,
        };
        let c = char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut utf8 = [0u8; 4];
        self.push_str(c.encode_utf8(&mut utf8).as_bytes(), at, sink)
    }
}

/// The scalar value of a surrogate pair.
// ARITH: `high` is in D800..=DBFF and `low` in DC00..=DFFF (the callers'
// match arms), so neither subtraction wraps and the sum is at most 0x10FFFF.
#[allow(clippy::arithmetic_side_effects)]
fn combine(high: u32, low: u32) -> u32 {
    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
}
