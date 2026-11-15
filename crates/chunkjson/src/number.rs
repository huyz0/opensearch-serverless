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

impl Tokenizer {
    // ARITH: `j` only advances past a byte read with `input.get(j)`.
    #[allow(clippy::arithmetic_side_effects)]
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
        }
    }

    #[test]
    fn refuses_what_json_refuses() {
        for bad in [
            "01", "-01", "-a", "1.e3", "1.", ".1", "1e", "1e+", "--1", "+1",
        ] {
            assert_ne!(lex(bad), Some(true), "{bad}");
        }
        assert_eq!(lex("1x"), Some(false), "an unknown byte ends the number");
        assert_eq!(lex("1.x"), None);
        assert_eq!(lex("1ex"), None);
        assert_eq!(lex("1e+x"), None);
        assert_eq!(lex("-x"), None);
        assert_eq!(lex("x"), None);
    }
}
