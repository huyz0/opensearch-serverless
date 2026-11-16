//! Lexing `true`, `false` and `null`, which may be split across slices.

use crate::tokenizer::{Lex, Out, Tokenizer};
use crate::{Error, ErrorKind, Event};

impl Tokenizer {
    // ARITH: `j` only advances past a byte read with `input.get(j)`, and
    // `matched` stays below `word.len()` (at most 5) before each increment.
    #[allow(clippy::arithmetic_side_effects)]
    #[inline]
    pub(crate) fn lex_literal(
        &mut self,
        input: &[u8],
        start: usize,
        eof: bool,
    ) -> Result<(Out, usize), Error> {
        let Lex::Lit { word, mut matched } = self.lex else {
            return Err(self.err(ErrorKind::UnexpectedEof, start));
        };
        let mut j = start;
        while let Some(&want) = word.get(matched) {
            let Some(&b) = input.get(j) else {
                if eof {
                    return Err(self.err(ErrorKind::UnexpectedEof, j));
                }
                self.lex = Lex::Lit { word, matched };
                return Ok((Out::Simple(Event::NeedMore), j));
            };
            if b != want {
                return Err(self.err(ErrorKind::UnexpectedByte(b), j));
            }
            matched += 1;
            j += 1;
        }
        self.lex = Lex::None;
        let event = match word {
            b"true" => Event::Bool(true),
            b"false" => Event::Bool(false),
            _ => Event::Null,
        };
        self.after_value(j, true);
        Ok((Out::Simple(event), j))
    }
}
