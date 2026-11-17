//! A `_bulk` body read as it streams: lines from `ndjson::Splitter`, each
//! action line through the tokenizer, each source line kept whole for the
//! engine. Operations come out whole, a slice at a time, as the shell's
//! `BulkBodyParser.parsePrefix` hands them out; the body is never held.

use oss_rest::json::{Limits, Tokenizer};
use oss_rest::ndjson::{LineSink, Splitter};
use oss_rest::{Exception, RestError};

use super::action::{Action, ActionReader, Defaults, Kind};

/// One operation of the body: its action and, but for a delete, its source.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// The action line's metadata.
    pub action: Action,
    /// The source line, without its newline; empty for a delete.
    pub source: Vec<u8>,
}

impl Op {
    /// The heap the operation holds, as the budget counts it.
    pub fn bytes(&self) -> usize {
        let a = &self.action;
        let text = |s: &Option<String>| s.as_ref().map_or(0, String::len);
        self.source
            .len()
            .saturating_add(text(&a.index))
            .saturating_add(text(&a.id))
            .saturating_add(text(&a.routing))
            .saturating_add(text(&a.pipeline))
            .saturating_add(std::mem::size_of::<Op>())
    }
}

/// What the next line is.
enum Next {
    /// An action line (or a blank one).
    Action,
    /// The source line of this action.
    Source(Action),
}

/// The line sink: the state between lines.
struct Lines<'d> {
    defaults: &'d Defaults,
    tokenizer: Tokenizer,
    next: Next,
    current: Option<ActionReader<'d>>,
    source: Vec<u8>,
    ops: Vec<Op>,
    held: usize,
}

impl<'d> LineSink for Lines<'d> {
    type Error = RestError;

    fn part(&mut self, bytes: &[u8]) -> Result<(), RestError> {
        match &self.next {
            Next::Source(_) => {
                // ALLOC: one source line; bounded by the slice's grant of the bulk budget.
                self.source.extend_from_slice(bytes);
                Ok(())
            }
            Next::Action => {
                let Some(reader) = self.current.as_mut() else {
                    return Ok(());
                };
                if reader.is_done() {
                    // `BulkRequestParser` reads no further than the action's object.
                    return Ok(());
                }
                match self.tokenizer.feed(bytes, reader) {
                    // `BulkRequestParser` stops at the action's closing brace,
                    // so what follows it, malformed or not, is never read.
                    Err(_) if reader.is_done() => Ok(()),
                    other => other,
                }
            }
        }
    }

    fn end_line(&mut self, line: u64) -> Result<(), RestError> {
        match std::mem::replace(&mut self.next, Next::Action) {
            Next::Source(action) => {
                let source = std::mem::take(&mut self.source);
                self.push(Op { action, source });
            }
            Next::Action => {
                if let Some(mut reader) = self.current.take() {
                    if !reader.is_done() {
                        self.tokenizer.finish(&mut reader)?;
                    }
                    self.tokenizer.reset();
                    if let Some(action) = reader.take() {
                        if action.kind == Kind::Delete {
                            self.push(Op {
                                action,
                                // ALLOC: none; an empty vector does not allocate.
                                source: Vec::new(),
                            });
                        } else {
                            self.next = Next::Source(action);
                        }
                    }
                }
            }
        }
        if matches!(self.next, Next::Action) {
            self.current = Some(ActionReader::new(line.saturating_add(1), self.defaults));
        }
        Ok(())
    }
}

impl Lines<'_> {
    fn push(&mut self, op: Op) {
        self.held = self.held.saturating_add(op.bytes());
        // ALLOC: one operation; the slice is bounded by its grant of the bulk budget.
        self.ops.push(op);
    }
}

/// Reads a `_bulk` body fed in chunks.
pub struct BodyReader<'d> {
    splitter: Splitter,
    lines: Lines<'d>,
    fed: bool,
}

impl<'d> BodyReader<'d> {
    /// A reader over a body whose action lines default to `defaults`.
    pub fn new(defaults: &'d Defaults) -> Self {
        Self {
            splitter: Splitter::new(),
            lines: Lines {
                defaults,
                tokenizer: Tokenizer::new(Limits::default()),
                next: Next::Action,
                current: Some(ActionReader::new(1, defaults)),
                // ALLOC: none until a source line arrives.
                source: Vec::new(),
                // ALLOC: none until an operation is whole.
                ops: Vec::new(),
                held: 0,
            },
            fed: false,
        }
    }

    /// Feeds the next chunk of the body.
    ///
    /// # Errors
    ///
    /// A malformed action line, as the Java refuses it; the request ends.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), RestError> {
        if !chunk.is_empty() {
            self.fed = true;
        }
        self.splitter.feed(chunk, &mut self.lines)
    }

    /// The heap the operations read so far hold.
    pub fn held(&self) -> usize {
        self.lines.held
    }

    /// The whole operations read so far, in body order, emptying the
    /// reader's slice.
    pub fn take(&mut self) -> Vec<Op> {
        self.lines.held = 0;
        std::mem::take(&mut self.lines.ops)
    }

    /// Ends the body: a body of no bytes, or one not ending in a newline, is
    /// refused. An action whose source line never came is dropped, as
    /// `BulkRequestParser` drops it.
    ///
    /// # Errors
    ///
    /// `request body is required`, or the newline refusal.
    pub fn finish(&mut self) -> Result<(), RestError> {
        if !self.fed {
            return Err(RestError::exception(
                400,
                Exception::new("parse_exception", "request body is required"),
            ));
        }
        self.splitter.finish()?;
        self.lines.next = Next::Action;
        Ok(())
    }
}
