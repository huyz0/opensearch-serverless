//! String and key lexing. A string streams out as fragments: each run of
//! unescaped bytes is borrowed from the caller's slice, and each escape or
//! character split across slices is decoded into the tokenizer's 4-byte buffer.

use crate::tokenizer::{Lex, Out, Src, Tokenizer};
use crate::{Error, ErrorKind, Event};

/// Progress through a backslash escape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Esc {
    /// Not in an escape.
    None,
    /// After `\`; `high` is a pending high surrogate.
    Backslash { high: Option<u16> },
    /// In `\uXXXX`, after `n` hex digits worth `val`.
    Hex { n: u8, val: u16, high: Option<u16> },
    /// After a high surrogate: its low half's `\` must follow.
    LowBackslash { high: u16 },
}

/// The state of a string being lexed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StrState {
    key: bool,
    /// No fragment emitted yet.
    first: bool,
    esc: Esc,
}

impl StrState {
    pub(crate) fn new(key: bool) -> Self {
        Self {
            key,
            first: true,
            esc: Esc::None,
        }
    }
}

/// True for the bytes that end a run of plain string bytes.
#[inline]
fn is_special(b: u8) -> bool {
    b == b'"' || b == b'\\' || b < 0x20
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGHS: u64 = 0x8080_8080_8080_8080;

/// Index of the first byte at or after `j` that ends a run of plain string
/// bytes, or `input.len()`. Eight bytes at a time (SWAR): a byte is flagged
/// when it equals `"` or `\` or is below 0x20. The tricks can also flag a
/// byte above a true hit (a borrow carries upward), never below one, so the
/// lowest flag is exact.
// ARITH: `j + 8 <= input.len()` is checked before each step of 8, and the
// tail loop advances past bytes read with `get`.
#[allow(clippy::arithmetic_side_effects)]
pub(crate) fn scan_plain(input: &[u8], mut j: usize) -> usize {
    while let Some(word) = input.get(j..j + 8) {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(word);
        let x = u64::from_le_bytes(bytes);
        let quote = x ^ (ONES * u64::from(b'"'));
        let slash = x ^ (ONES * u64::from(b'\\'));
        let zero = |v: u64| v.wrapping_sub(ONES) & !v;
        let control = x.wrapping_sub(ONES * 0x20) & !x;
        let hits = (zero(quote) | zero(slash) | control) & HIGHS;
        if hits != 0 {
            return j + (hits.trailing_zeros() / 8) as usize;
        }
        j += 8;
    }
    while let Some(&b) = input.get(j) {
        if is_special(b) {
            break;
        }
        j += 1;
    }
    j
}

/// The UTF-8 length a lead byte announces, or 0 if `b` is not a multi-byte lead.
fn utf8_width(b: u8) -> usize {
    match b {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 0,
    }
}

/// If `run` ends inside a multi-byte character, the number of bytes of that
/// partial character and the character's full width.
// ARITH: `k` runs from 1 to at most 3 and is at most `run.len()`.
#[allow(clippy::arithmetic_side_effects)]
fn split_tail(run: &[u8]) -> Option<(usize, usize)> {
    for k in 1..=run.len().min(3) {
        let b = run[run.len() - k];
        if b & 0xC0 == 0x80 {
            continue;
        }
        let width = utf8_width(b);
        return (width > k).then_some((k, width));
    }
    None
}

fn hex(b: u8) -> Option<u16> {
    (b as char).to_digit(16).and_then(|d| u16::try_from(d).ok())
}

fn simple_escape(b: u8) -> Option<char> {
    Some(match b {
        b'"' => '"',
        b'\\' => '\\',
        b'/' => '/',
        b'b' => '\u{8}',
        b'f' => '\u{c}',
        b'n' => '\n',
        b'r' => '\r',
        b't' => '\t',
        _ => return None,
    })
}

impl Tokenizer {
    /// Lexes a string from index `j` of `input`, returning its next fragment.
    // `j` only advances past a byte read with `input.get(j)`; `utf8_len` stays
    // below `utf8_need` (at most 4); `n` counts at most four hex digits.
    // ARITH: so no index overflows, and `val << 4 | d` after three digits is
    // below 0x1000, so the shift keeps it in a u16.
    #[allow(clippy::arithmetic_side_effects)]
    pub(crate) fn lex_string(
        &mut self,
        input: &[u8],
        mut j: usize,
        eof: bool,
    ) -> Result<(Out, usize), Error> {
        let Lex::Str(mut st) = self.lex else {
            return Err(self.err(ErrorKind::UnexpectedEof, j));
        };
        loop {
            if self.utf8_len > 0 {
                while self.utf8_len < self.utf8_need {
                    let Some(&b) = input.get(j) else {
                        return self.string_need_more(st, j, eof);
                    };
                    if b & 0xC0 != 0x80 {
                        return Err(Error::new(ErrorKind::InvalidUtf8, self.utf8_at));
                    }
                    self.utf8[self.utf8_len] = b;
                    self.utf8_len += 1;
                    j += 1;
                }
                let bytes = &self.utf8[..self.utf8_need];
                let Some(c) = std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|s| s.chars().next())
                else {
                    return Err(Error::new(ErrorKind::InvalidUtf8, self.utf8_at));
                };
                self.utf8_len = 0;
                return Ok(self.emit_char(st, c, j));
            }
            if st.esc != Esc::None {
                let Some(&b) = input.get(j) else {
                    return self.string_need_more(st, j, eof);
                };
                let at = j;
                j += 1;
                match st.esc {
                    Esc::Backslash { high } => {
                        if b == b'u' {
                            st.esc = Esc::Hex { n: 0, val: 0, high };
                            continue;
                        }
                        if high.is_some() {
                            return Err(self.err(ErrorKind::LoneSurrogate, at));
                        }
                        let Some(c) = simple_escape(b) else {
                            return Err(self.err(ErrorKind::InvalidEscape, at));
                        };
                        st.esc = Esc::None;
                        return Ok(self.emit_char(st, c, j));
                    }
                    Esc::Hex { n, val, high } => {
                        let Some(d) = hex(b) else {
                            return Err(self.err(ErrorKind::InvalidEscape, at));
                        };
                        let (n, val) = (n + 1, val << 4 | d);
                        if n < 4 {
                            st.esc = Esc::Hex { n, val, high };
                            continue;
                        }
                        let code = match (val, high) {
                            (0xD800..=0xDBFF, None) => {
                                st.esc = Esc::LowBackslash { high: val };
                                continue;
                            }
                            (0xDC00..=0xDFFF, Some(h)) => {
                                0x10000
                                    + ((u32::from(h) - 0xD800) << 10)
                                    + (u32::from(val) - 0xDC00)
                            }
                            (0xDC00..=0xDFFF, None) | (_, Some(_)) => {
                                return Err(self.err(ErrorKind::LoneSurrogate, at));
                            }
                            (v, None) => u32::from(v),
                        };
                        let Some(c) = char::from_u32(code) else {
                            return Err(self.err(ErrorKind::LoneSurrogate, at));
                        };
                        st.esc = Esc::None;
                        return Ok(self.emit_char(st, c, j));
                    }
                    Esc::LowBackslash { high } => {
                        if b != b'\\' {
                            return Err(self.err(ErrorKind::LoneSurrogate, at));
                        }
                        st.esc = Esc::Backslash { high: Some(high) };
                        continue;
                    }
                    Esc::None => {}
                }
            }
            let start = j;
            j = scan_plain(input, j);
            match input.get(j) {
                None => {
                    let mut end = j;
                    if let Some((partial, width)) = split_tail(&input[start..j]) {
                        end = j - partial;
                        self.utf8[..partial].copy_from_slice(&input[end..j]);
                        self.utf8_len = partial;
                        self.utf8_need = width;
                        self.utf8_at = self.base.saturating_add(end as u64);
                    }
                    if end > start {
                        return Ok(self.emit(st, Src::Input(start, end), false, j));
                    }
                    return self.string_need_more(st, j, eof);
                }
                Some(b'"') => {
                    self.lex = Lex::None;
                    if st.key {
                        self.after_key();
                    } else {
                        self.after_value(j + 1);
                    }
                    let out = Out::Text {
                        key: st.key,
                        src: Src::Input(start, j),
                        first: st.first,
                        last: true,
                    };
                    return Ok((out, j + 1));
                }
                Some(b'\\') if j > start => {
                    return Ok(self.emit(st, Src::Input(start, j), false, j));
                }
                Some(b'\\') => {
                    st.esc = Esc::Backslash { high: None };
                    j += 1;
                }
                Some(_) => return Err(self.err(ErrorKind::ControlCharacter, j)),
            }
        }
    }

    fn string_need_more(
        &mut self,
        st: StrState,
        j: usize,
        eof: bool,
    ) -> Result<(Out, usize), Error> {
        if eof {
            return Err(self.err(ErrorKind::UnexpectedEof, j));
        }
        self.lex = Lex::Str(st);
        Ok((Out::Simple(Event::NeedMore), j))
    }

    fn emit_char(&mut self, st: StrState, c: char, used: usize) -> (Out, usize) {
        self.char_len = c.encode_utf8(&mut self.char_buf).len();
        self.emit(st, Src::Char, false, used)
    }

    /// Emits a non-final fragment and saves the string state for the next call.
    fn emit(&mut self, mut st: StrState, src: Src, last: bool, used: usize) -> (Out, usize) {
        let out = Out::Text {
            key: st.key,
            src,
            first: st.first,
            last,
        };
        st.first = false;
        self.lex = Lex::Str(st);
        (out, used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_plain_stops_at_the_first_special_byte_in_any_lane() {
        for special in [b'"', b'\\', 0x00, 0x1f, b'\n'] {
            for at in 0..20 {
                let mut bytes = vec![b'a'; 20];
                bytes[at] = special;
                // a later special byte must not hide the first
                if at + 1 < 20 {
                    bytes[at + 1] = b'"';
                }
                assert_eq!(scan_plain(&bytes, 0), at, "{special:#x} at {at}");
            }
        }
        let plain = "aé€😀 \x7f\u{80}~!".repeat(3);
        assert_eq!(scan_plain(plain.as_bytes(), 0), plain.len());
        assert_eq!(scan_plain(b"ab\"", 1), 2);
        assert_eq!(scan_plain(b"", 0), 0);
    }
}
