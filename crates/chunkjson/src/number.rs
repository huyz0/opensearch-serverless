//! The JSON number grammar as a byte-at-a-time state machine, so a number can
//! be split across slices: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.

use crate::tokenizer::{Lex, Out, Src, Tokenizer};
use crate::{Error, ErrorKind, Event};

/// Where in the number grammar the lexer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumState {
    /// Nothing read yet.
    Start,
    /// After `-`.
    Minus,
    /// After a leading `0`.
    Zero,
    /// In the integer digits.
    Int,
    /// After `.`.
    Dot,
    /// In the fraction digits.
    Frac,
    /// After `e` or `E`.
    E,
    /// After the exponent's sign.
    ESign,
    /// In the exponent digits.
    Exp,
}

/// The result of offering one byte to the number lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The byte belongs to the number.
    Next(NumState),
    /// The byte does not belong to the number, and the number may end here
    /// (whether it is complete is [`NumState::is_final`]).
    End,
    /// The byte makes the number invalid.
    Invalid,
}

impl NumState {
    /// Offers byte `b`.
    #[inline]
    pub(crate) fn advance(self, b: u8) -> Step {
        use NumState::*;
        let digit = b.is_ascii_digit();
        let next = match (self, b) {
            (Start, b'-') => Minus,
            (Start | Minus, b'0') => Zero,
            (Start | Minus, _) if digit => Int,
            (Zero, _) if digit => return Step::Invalid,
            (Int, _) if digit => Int,
            (Zero | Int, b'.') => Dot,
            (Dot | Frac, _) if digit => Frac,
            (Zero | Int | Frac, b'e' | b'E') => E,
            (E, b'+' | b'-') => ESign,
            (E | ESign | Exp, _) if digit => Exp,
            (Zero | Int | Frac | Exp, _) => return Step::End,
            (Start | Minus | Dot | E | ESign, _) => return Step::Invalid,
        };
        Step::Next(next)
    }

    /// The number read so far is complete.
    pub(crate) fn is_final(self) -> bool {
        matches!(
            self,
            NumState::Zero | NumState::Int | NumState::Frac | NumState::Exp
        )
    }
}

/// `s` is exactly one JSON number.
// ARITH: `i` plus a count of bytes after it is at most `b.len()`.
#[allow(clippy::arithmetic_side_effects)]
pub(crate) fn is_number(s: &str) -> bool {
    let b = s.as_bytes();
    let digits_from = |i: usize| {
        let run = b.get(i..).unwrap_or_default();
        i + run.iter().take_while(|c| c.is_ascii_digit()).count()
    };
    let mut i = usize::from(b.first() == Some(&b'-'));
    i = match b.get(i) {
        Some(b'0') => i + 1,
        Some(b'1'..=b'9') => digits_from(i),
        _ => return false,
    };
    if b.get(i) == Some(&b'.') {
        let end = digits_from(i + 1);
        if end == i + 1 {
            return false;
        }
        i = end;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let end = digits_from(i);
        if end == i {
            return false;
        }
        i = end;
    }
    i == b.len()
}

impl Tokenizer {
    // ARITH: `j` only advances past a byte read with `input.get(j)`.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    pub(crate) fn lex_number(
        &mut self,
        input: &[u8],
        start: usize,
        eof: bool,
    ) -> Result<(Out, usize), Error> {
        let Lex::Num(mut state) = self.lex else {
            return Err(self.err(ErrorKind::InvalidNumber, start));
        };
        let mut j = start;
        while let Some(&b) = input.get(j) {
            if b.is_ascii_digit() && matches!(state, NumState::Int | NumState::Frac | NumState::Exp)
            {
                j += 1;
                continue;
            }
            match state.advance(b) {
                Step::Next(next) => {
                    state = next;
                    j += 1;
                }
                Step::End => break,
                Step::Invalid => return Err(self.err(ErrorKind::InvalidNumber, j)),
            }
        }
        let bytes = input.get(start..j).unwrap_or_default();
        let ended = j < input.len() || eof;
        let src = if self.numbuf.is_empty() && ended {
            if bytes.len() > self.config.max_number_len {
                return Err(self.err(
                    ErrorKind::NumberTooLong,
                    start.saturating_add(self.config.max_number_len),
                ));
            }
            Src::Input(start, j)
        } else {
            let room = self.config.max_number_len.saturating_sub(self.numbuf.len());
            if bytes.len() > room {
                return Err(self.err(ErrorKind::NumberTooLong, start.saturating_add(room)));
            }
            self.numbuf.extend_from_slice(bytes);
            Src::NumBuf
        };
        if !ended {
            self.lex = Lex::Num(state);
            return Ok((Out::Simple(Event::NeedMore), j));
        }
        if !state.is_final() {
            return Err(self.err(ErrorKind::InvalidNumber, j));
        }
        self.lex = Lex::None;
        self.after_value(j, true);
        Ok((Out::Number(src), j))
    }
}

/// The end of a complete, valid number starting at `chunk[pos]`, if a byte
/// that cannot continue it follows inside the chunk. `None` leaves the
/// number to the general machine, which also reports malformed ones.
// ARITH: every index is a position found inside `chunk` plus at most one.
#[allow(clippy::arithmetic_side_effects)]
#[inline]
pub(crate) fn number_end(chunk: &[u8], pos: usize) -> Option<usize> {
    let mut j = pos;
    if chunk.get(j) == Some(&b'-') {
        j += 1;
    }
    j = match chunk.get(j)? {
        b'0' => j + 1,
        b'1'..=b'9' => digits(chunk, j)?,
        _ => return None,
    };
    if chunk.get(j) == Some(&b'.') {
        j = at_least_one_digit(chunk, j + 1)?;
    }
    if matches!(chunk.get(j), Some(b'e' | b'E')) {
        j += 1;
        if matches!(chunk.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        j = at_least_one_digit(chunk, j)?;
    }
    match chunk.get(j)? {
        b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => None,
        _ => Some(j),
    }
}

/// The index of the first non-digit at or after `j`, if the chunk has one.
// ARITH: `j + n` is at most `chunk.len()`.
#[allow(clippy::arithmetic_side_effects)]
#[inline]
fn digits(chunk: &[u8], j: usize) -> Option<usize> {
    let rest = chunk.get(j..)?;
    let mut words = rest.chunks_exact(8);
    let mut seen = 0;
    for word in words.by_ref() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(word);
        let lanes = non_digit_lanes(u64::from_le_bytes(bytes));
        if lanes != 0 {
            return Some(j + seen + (lanes.trailing_zeros() / 8) as usize);
        }
        seen += 8;
    }
    let tail = words.remainder();
    Some(j + seen + tail.iter().position(|b| !b.is_ascii_digit())?)
}

/// The high bit of every byte lane of `x` that is not an ASCII digit.
#[inline]
fn non_digit_lanes(x: u64) -> u64 {
    const LANES: u64 = 0x0101_0101_0101_0101;
    // A digit is 0x30-0x39: high nibble 3, low nibble at most 9. Adding 6
    // to a low nibble carries into bit 4 only for 10-15, and never into the
    // next lane.
    let high = (x & (LANES * 0xF0)) ^ (LANES * 0x30);
    let low = (x & (LANES * 0x0F)).wrapping_add(LANES * 0x06) & (LANES * 0x10);
    let v = high | low;
    // a lane of `v` is non-zero exactly when its byte is not a digit
    ((v & (LANES * 0x7F)).wrapping_add(LANES * 0x7F) | v) & (LANES * 0x80)
}

/// As [`digits`], but `None` unless at least one digit is there.
#[inline]
fn at_least_one_digit(chunk: &[u8], j: usize) -> Option<usize> {
    digits(chunk, j).filter(|&end| end > j)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(text: &str) -> Option<bool> {
        let mut state = NumState::Start;
        for &b in text.as_bytes() {
            match state.advance(b) {
                Step::Next(next) => state = next,
                Step::End => return Some(false),
                Step::Invalid => return None,
            }
        }
        Some(state.is_final())
    }

    #[test]
    fn accepts_the_json_grammar() {
        for ok in [
            "0",
            "-0",
            "7",
            "-12",
            "0.5",
            "1.25e10",
            "1E+2",
            "1e-2",
            "-0.0e0",
            "123456789012345678901234567890",
        ] {
            assert_eq!(lex(ok), Some(true), "{ok}");
            assert!(is_number(ok), "{ok}");
        }
    }

    #[test]
    fn refuses_what_json_refuses() {
        for bad in [
            "01", "-01", "-a", "1.e3", "1.", ".1", "1e", "1e+", "--1", "+1", "", "-", "1 ", "1x",
            "1.5.3", "1e5e", "0x1",
        ] {
            assert_ne!(lex(bad), Some(true), "{bad}");
            assert!(!is_number(bad), "{bad}");
        }
        assert_eq!(lex("1x"), Some(false), "an unknown byte ends the number");
        assert_eq!(lex("1.x"), None);
        assert_eq!(lex("1ex"), None);
        assert_eq!(lex("1e+x"), None);
        assert_eq!(lex("-x"), None);
        assert_eq!(lex("x"), None);
    }

    #[test]
    fn a_number_cut_by_the_chunk_end_is_left_to_the_general_machine() {
        assert_eq!(number_end(b"12.5,", 0), Some(4));
        assert_eq!(number_end(b"-0e+7]", 0), Some(5));
        assert_eq!(number_end(b"12", 0), None, "may continue in the next chunk");
        assert_eq!(number_end(b"1.x", 0), None);
        assert_eq!(number_end(b"[7,", 1), Some(2));
        assert_eq!(number_end(b"0,", 0), Some(1));
        assert_eq!(number_end(b"1E+2]", 0), Some(4));
        assert_eq!(number_end(b"-12.25e-3 ", 0), Some(9));
        assert_eq!(
            number_end(b"01,", 0),
            None,
            "malformed: the general machine reports it"
        );
        assert_eq!(number_end(b"1.5.", 0), None);
        assert_eq!(number_end(b"-,", 0), None);
        assert_eq!(number_end(b"1e,", 0), None);
        assert_eq!(number_end(b"1.,", 0), None);
        assert_eq!(number_end(b"-", 0), None);
        assert_eq!(number_end(b"1e", 0), None);
    }

    #[test]
    fn digit_lanes_flag_every_non_digit_byte() {
        for b in 0..=255u8 {
            for lane in 0..8 {
                let mut bytes = *b"01234567";
                bytes[lane] = b;
                let lanes = non_digit_lanes(u64::from_le_bytes(bytes));
                let want = if b.is_ascii_digit() {
                    0
                } else {
                    0x80 << (8 * lane)
                };
                assert_eq!(lanes, want, "byte {b:#x} in lane {lane}");
            }
        }
    }

    #[test]
    fn digit_runs_end_at_the_first_non_digit_or_are_cut() {
        assert_eq!(digits(b"123456789012,", 0), Some(12));
        assert_eq!(digits(b"x12345678,", 1), Some(9));
        assert_eq!(digits(b"1234567,", 0), Some(7));
        assert_eq!(digits(b",", 0), Some(0));
        assert_eq!(
            digits(b"123456789", 0),
            None,
            "may continue in the next chunk"
        );
        assert_eq!(digits(b"12345678", 0), None);
        assert_eq!(digits(b"1", 2), None);
        assert_eq!(at_least_one_digit(b"x", 0), None);
        assert_eq!(at_least_one_digit(b"1x", 0), Some(1));
    }
}
