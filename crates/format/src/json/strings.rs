//! JSON strings: scanning, escapes, and checking a skipped string without
//! decoding it.

use std::borrow::Cow;

use super::Reader;
use crate::error::FormatError;

impl<'a> Reader<'a> {
    /// A string's content, after its opening quote; leaves the reader past the
    /// closing one.
    pub(super) fn string(&mut self) -> Result<Cow<'a, str>, FormatError> {
        let start = self.at;
        loop {
            match self.bytes.get(self.at) {
                None => return Err(self.truncated()),
                Some(b'"') => {
                    let raw = self.bytes.get(start..self.at).unwrap_or_default();
                    self.bump();
                    return std::str::from_utf8(raw)
                        .map(Cow::Borrowed)
                        .map_err(|_| self.corrupt("a string is not UTF-8"));
                }
                Some(b'\\') if !self.decode => return self.scan_escaped(start),
                Some(b'\\') => return self.escaped(start).map(Cow::Owned),
                Some(b) if *b < 0x20 => {
                    return Err(self.corrupt("a string holds a control character"));
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// The rest of a string that holds an escape, checked as [`Reader::escaped`]
    /// checks it, and returned raw, escapes undecoded, without a copy.
    fn scan_escaped(&mut self, start: usize) -> Result<Cow<'a, str>, FormatError> {
        loop {
            match self.bytes.get(self.at) {
                None => return Err(self.truncated()),
                Some(b'"') => {
                    let raw = self.bytes.get(start..self.at).unwrap_or_default();
                    self.bump();
                    return std::str::from_utf8(raw)
                        .map(Cow::Borrowed)
                        .map_err(|_| self.corrupt("a string is not UTF-8"));
                }
                Some(b'\\') => {
                    self.bump();
                    self.escape()?;
                }
                Some(b) if *b < 0x20 => {
                    return Err(self.corrupt("a string holds a control character"));
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// The rest of a string that holds an escape, decoded into a new string.
    fn escaped(&mut self, start: usize) -> Result<String, FormatError> {
        let plain = self.bytes.get(start..self.at).unwrap_or_default();
        let plain =
            std::str::from_utf8(plain).map_err(|_| self.corrupt("a string is not UTF-8"))?;
        // ALLOC: one string per escaped JSON string, no longer than the record the caller bounded.
        let mut out = String::with_capacity(plain.len().saturating_add(16));
        out.push_str(plain);
        loop {
            let run = self.at;
            while matches!(self.bytes.get(self.at), Some(b) if *b != b'"' && *b != b'\\' && *b >= 0x20)
            {
                self.bump();
            }
            let text = self.bytes.get(run..self.at).unwrap_or_default();
            out.push_str(
                std::str::from_utf8(text).map_err(|_| self.corrupt("a string is not UTF-8"))?,
            );
            match self.bytes.get(self.at) {
                None => return Err(self.truncated()),
                Some(b'"') => {
                    self.bump();
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.bump();
                    let decoded = self.escape()?;
                    out.push(decoded);
                }
                Some(_) => return Err(self.corrupt("a string holds a control character")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, FormatError> {
        let Some(b) = self.bytes.get(self.at).copied() else {
            return Err(self.truncated());
        };
        self.bump();
        Ok(match b {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let high = self.hex4()?;
                if (0xD800..0xDC00).contains(&high) {
                    if self.bytes.get(self.at..self.at.saturating_add(2)) != Some(b"\\u") {
                        return Err(self.corrupt("a string holds a lone surrogate"));
                    }
                    self.at = self.at.saturating_add(2);
                    let low = self.hex4()?;
                    if !(0xDC00..0xE000).contains(&low) {
                        return Err(self.corrupt("a string holds a lone surrogate"));
                    }
                    let units = [
                        u16::try_from(high).unwrap_or_default(),
                        u16::try_from(low).unwrap_or_default(),
                    ];
                    char::decode_utf16(units)
                        .next()
                        .and_then(Result::ok)
                        .ok_or_else(|| self.corrupt("a string holds a lone surrogate"))?
                } else {
                    char::from_u32(high)
                        .ok_or_else(|| self.corrupt("a string holds a lone surrogate"))?
                }
            }
            _ => return Err(self.corrupt("a string holds an unknown escape")),
        })
    }

    fn hex4(&mut self) -> Result<u32, FormatError> {
        let end = self.at.saturating_add(4);
        let Some(digits) = self.bytes.get(self.at..end) else {
            return Err(self.truncated());
        };
        let mut value = 0u32;
        for d in digits {
            let nibble = char::from(*d)
                .to_digit(16)
                .ok_or_else(|| self.corrupt("a \\u escape is not hex"))?;
            value = (value << 4) | nibble;
        }
        self.at = end;
        Ok(value)
    }
}
