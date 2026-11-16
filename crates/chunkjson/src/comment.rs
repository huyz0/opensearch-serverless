//! Comments, when `Config::comments` allows them, and positions: the line
//! count kept at line breaks between tokens, and where each event starts.

use crate::tokenizer::{Lex, Out, Tokenizer};
use crate::{Error, ErrorKind, Event, Framing, Position};

/// Where in a comment the lexer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Comment {
    /// After the opening `/`.
    Slash,
    /// In a `//` comment, which the next line break ends.
    Line,
    /// In a `/* */` comment.
    Block,
    /// In a block comment, just after a `*`.
    BlockStar,
}

impl Tokenizer {
    /// Lexes a comment from `input[*i]`. Returns `None` once the comment is
    /// over, with `*i` just past it (a line comment leaves its line break
    /// for the caller), or the step's result: `NeedMore` or an error.
    // ARITH: `*i` only advances past a byte read with `input.get(*i)`, so it
    // stays at most `input.len()`.
    #[allow(clippy::arithmetic_side_effects)]
    pub(crate) fn lex_comment(
        &mut self,
        mut state: Comment,
        input: &[u8],
        i: &mut usize,
        eof: bool,
    ) -> Result<Option<(Out, usize)>, Error> {
        // each pass reads one byte or returns, so the loop is bounded
        for _ in 0..=input.len() {
            let Some(&b) = input.get(*i) else {
                break;
            };
            state = match (state, b) {
                (Comment::Slash, b'/') => Comment::Line,
                (Comment::Slash, b'*') => Comment::Block,
                (Comment::Slash, _) => {
                    let kind = ErrorKind::UnexpectedByte(b'/');
                    return Err(self.err_abs(kind, self.comment_at));
                }
                (Comment::Line, b'\n') => {
                    self.lex = Lex::None;
                    return Ok(None);
                }
                (Comment::Line, _) => Comment::Line,
                (Comment::Block | Comment::BlockStar, b'*') => Comment::BlockStar,
                (Comment::BlockStar, b'/') => {
                    self.lex = Lex::None;
                    *i += 1;
                    return Ok(None);
                }
                (Comment::Block | Comment::BlockStar, b'\n') => {
                    // NDJSON splits at every line break, so a block comment
                    // may not span one, inside a document or between two.
                    if self.config.framing == Framing::Ndjson {
                        return Err(self.err(ErrorKind::NewlineInDocument, *i));
                    }
                    self.newline(*i + 1);
                    Comment::Block
                }
                (Comment::Block | Comment::BlockStar, _) => Comment::Block,
            };
            *i += 1;
        }
        if !eof {
            self.lex = Lex::Comment(state);
            return Ok(Some((Out::Simple(Event::NeedMore), *i)));
        }
        if state == Comment::Line {
            self.lex = Lex::None;
            return Ok(None);
        }
        Err(self.err(ErrorKind::UnexpectedEof, *i))
    }

    /// A line break ended just before index `i` of the current input.
    #[inline]
    pub(crate) fn newline(&mut self, i: usize) {
        self.line = self.line.saturating_add(1);
        self.line_start = self.base.saturating_add(i as u64);
    }

    /// The start of the last event: what [`Tokenizer::event_start`] returns.
    #[inline]
    pub(crate) fn position(&self) -> Position {
        if self.doc_ended {
            return self.doc_pos;
        }
        Position {
            offset: self.start,
            line: self.line,
            column: self.start.saturating_sub(self.line_start).saturating_add(1),
        }
    }

    /// Where the last event returned by [`Tokenizer::next`] starts, once the
    /// event is dropped ([`Tokenizer::next_at`] returns it with the event): the
    /// opening quote of a key or string (for every fragment of it), the first
    /// byte of a number or literal, or the bracket. For `EndDocument`, the
    /// document's start.
    pub fn event_start(&self) -> Position {
        self.position()
    }

    /// An error at absolute `offset`, which is on the current line.
    pub(crate) fn err_abs(&self, kind: ErrorKind, offset: u64) -> Error {
        let column = offset.saturating_sub(self.line_start).saturating_add(1);
        Error::at(kind, offset, self.line, column)
    }
}
