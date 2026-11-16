//! Writing JSON: the mirror of the tokenizer.
//!
//! A [`Writer`] takes the events a [`Tokenizer`](crate::Tokenizer) yields, or
//! the building calls that stand for them, and writes compact JSON to an
//! [`Output`]. Strings go fragment by fragment, so one of any size passes
//! through in bounded memory. The writer refuses a call that would make the
//! output invalid JSON, writes nothing for it, and stays usable.

use std::convert::Infallible;
use std::{fmt, io};

use crate::escape::{decimal, escaped};
use crate::number::is_number;
use crate::{Config, Event, Framing, Text};

/// Where a [`Writer`] puts its bytes.
pub trait Output {
    /// The error a write can fail with.
    type Error;
    /// Writes all of `bytes`.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

impl Output for Vec<u8> {
    type Error = Infallible;
    fn write(&mut self, bytes: &[u8]) -> Result<(), Infallible> {
        // ALLOC: the caller's buffer; it grows by what is written, and an
        // async caller drains it between calls.
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// An [`Output`] over any [`std::io::Write`]. Each piece (a comma, a quote,
/// a run of plain text, an escape) is one `write_all`, so wrap an unbuffered
/// writer such as a file or socket in a [`std::io::BufWriter`].
#[derive(Debug)]
pub struct IoOutput<W>(pub W);

impl<W: io::Write> Output for IoOutput<W> {
    type Error = io::Error;
    fn write(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.0.write_all(bytes)
    }
}

/// A call the writer refused, because it would make the output invalid JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Misuse {
    /// A key where none may come: outside an object, or after a key.
    KeyNotAllowed,
    /// A value where none may come: where a key is due, or after the only
    /// document in single framing.
    ValueNotAllowed,
    /// An end that does not close the open container, or closes an object
    /// after a key.
    EndNotAllowed,
    /// A container, number, boolean or null while a string or key is open.
    StringOpen,
    /// A fragment that neither starts a string when none is open nor
    /// continues the open one.
    NotAContinuation,
    /// An array or object nested deeper than `Config::max_depth`.
    TooDeep,
    /// Number text that is not exactly one JSON number.
    InvalidNumber,
    /// `check_complete` with a container or string open, or with no document in
    /// single framing.
    Unfinished,
    /// An earlier write to the output failed, so the output is incomplete.
    Failed,
}

/// Why a [`Writer`] call failed.
#[derive(Debug)]
pub enum WriteError<E> {
    /// The call was refused and nothing was written.
    Misuse(Misuse),
    /// The output failed; the writer refuses every later call.
    Output(E),
}

type Done<O> = Result<(), WriteError<<O as Output>::Error>>;

/// A sans-IO JSON writer with a fixed nesting limit.
#[derive(Debug)]
pub struct Writer {
    framing: Framing,
    max_depth: usize,
    /// Open containers, innermost last: `true` for an object.
    stack: Vec<bool>,
    /// A value has been written in the innermost container.
    need_comma: bool,
    /// A key has been written and its value is due.
    after_key: bool,
    /// An open string: `Some(true)` for a key.
    open: Option<bool>,
    /// A top-level value has been written.
    done: bool,
    failed: bool,
}

impl Writer {
    /// A writer for `config`'s framing and nesting limit. This is its only
    /// allocation.
    pub fn new(config: Config) -> Self {
        Self {
            framing: config.framing,
            max_depth: config.max_depth,
            // ALLOC: `max_depth` flags, fixed by the config.
            stack: Vec::with_capacity(config.max_depth),
            need_comma: false,
            after_key: false,
            open: None,
            done: false,
            failed: false,
        }
    }

    /// Writes one tokenizer event. `EndDocument`, `NeedMore` and `Eof` write
    /// nothing: framing is the writer's own.
    pub fn event<O: Output>(&mut self, event: Event<'_>, out: &mut O) -> Done<O> {
        match event {
            Event::BeginObject => self.begin(true, out),
            Event::EndObject => self.end(true, out),
            Event::BeginArray => self.begin(false, out),
            Event::EndArray => self.end(false, out),
            Event::Key(t) => self.text(true, t, out),
            Event::String(t) => self.text(false, t, out),
            Event::Number(n) => self.number(n, out),
            Event::Bool(b) => self.bool(b, out),
            Event::Null => self.null(out),
            Event::EndDocument { .. } | Event::NeedMore | Event::Eof => Ok(()),
        }
    }

    /// `{`
    pub fn begin_object<O: Output>(&mut self, out: &mut O) -> Done<O> {
        self.begin(true, out)
    }

    /// `}`
    pub fn end_object<O: Output>(&mut self, out: &mut O) -> Done<O> {
        self.end(true, out)
    }

    /// `[`
    pub fn begin_array<O: Output>(&mut self, out: &mut O) -> Done<O> {
        self.begin(false, out)
    }

    /// `]`
    pub fn end_array<O: Output>(&mut self, out: &mut O) -> Done<O> {
        self.end(false, out)
    }

    /// A whole key.
    pub fn key<O: Output>(&mut self, key: &str, out: &mut O) -> Done<O> {
        self.text(true, whole(key), out)
    }

    /// A whole string value.
    pub fn string<O: Output>(&mut self, s: &str, out: &mut O) -> Done<O> {
        self.text(false, whole(s), out)
    }

    /// A number given as its JSON text, which must be exactly one number.
    pub fn number<O: Output>(&mut self, text: &str, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        if !is_number(text) {
            return Err(WriteError::Misuse(Misuse::InvalidNumber));
        }
        self.scalar(comma, b"", text.as_bytes(), out)
    }

    /// An unsigned integer.
    pub fn u64<O: Output>(&mut self, n: u64, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        let mut buf = [0u8; 20];
        let at = decimal(n, &mut buf);
        self.scalar(comma, b"", buf.get(at..).unwrap_or_default(), out)
    }

    /// A signed integer.
    pub fn i64<O: Output>(&mut self, n: i64, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        let mut buf = [0u8; 20];
        let at = decimal(n.unsigned_abs(), &mut buf);
        let sign: &[u8] = if n < 0 { b"-" } else { b"" };
        self.scalar(comma, sign, buf.get(at..).unwrap_or_default(), out)
    }

    /// `true` or `false`.
    pub fn bool<O: Output>(&mut self, b: bool, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        self.scalar(comma, b"", if b { b"true" } else { b"false" }, out)
    }

    /// `null`
    pub fn null<O: Output>(&mut self, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        self.scalar(comma, b"", b"null", out)
    }

    /// Whether the output so far is complete: no container or string open,
    /// and in single framing, one document written.
    pub fn check_complete(&self) -> Result<(), Misuse> {
        if self.failed {
            return Err(Misuse::Failed);
        }
        let empty = self.framing == Framing::Single && !self.done;
        if self.open.is_some() || !self.stack.is_empty() || empty {
            return Err(Misuse::Unfinished);
        }
        Ok(())
    }

    /// Whether a value may come next; `Ok(true)` if a comma goes first.
    fn value_slot(&self) -> Result<bool, Misuse> {
        if self.failed {
            return Err(Misuse::Failed);
        }
        if self.open.is_some() {
            return Err(Misuse::StringOpen);
        }
        match self.stack.last() {
            None if self.done && self.framing == Framing::Single => Err(Misuse::ValueNotAllowed),
            None => Ok(false),
            Some(false) => Ok(self.need_comma),
            Some(true) if self.after_key => Ok(false),
            Some(true) => Err(Misuse::ValueNotAllowed),
        }
    }

    /// Whether a key may come next; `Ok(true)` if a comma goes first.
    fn key_slot(&self) -> Result<bool, Misuse> {
        if self.failed {
            return Err(Misuse::Failed);
        }
        if self.open.is_some() {
            return Err(Misuse::StringOpen);
        }
        match self.stack.last() {
            Some(true) if !self.after_key => Ok(self.need_comma),
            _ => Err(Misuse::KeyNotAllowed),
        }
    }

    fn begin<O: Output>(&mut self, object: bool, out: &mut O) -> Done<O> {
        let comma = self.value_slot()?;
        if self.stack.len() >= self.max_depth {
            return Err(WriteError::Misuse(Misuse::TooDeep));
        }
        let open: &[u8] = if object { b"{" } else { b"[" };
        self.write_all(out, &[if comma { b"," } else { b"" }, open])?;
        self.stack.push(object);
        self.need_comma = false;
        self.after_key = false;
        Ok(())
    }

    fn end<O: Output>(&mut self, object: bool, out: &mut O) -> Done<O> {
        if self.failed {
            return Err(WriteError::Misuse(Misuse::Failed));
        }
        if self.open.is_some() {
            return Err(WriteError::Misuse(Misuse::StringOpen));
        }
        if self.stack.last() != Some(&object) || self.after_key {
            return Err(WriteError::Misuse(Misuse::EndNotAllowed));
        }
        let close: &[u8] = if object { b"}" } else { b"]" };
        // the container's end is the end of a value in its parent
        let line = self.stack.len() == 1 && self.framing != Framing::Single;
        self.write_all(out, &[close, if line { b"\n" } else { b"" }])?;
        self.stack.pop();
        self.value_done();
        Ok(())
    }

    /// One fragment of a key or string value.
    fn text<O: Output>(&mut self, key: bool, t: Text<'_>, out: &mut O) -> Done<O> {
        if self.failed {
            return Err(WriteError::Misuse(Misuse::Failed));
        }
        match self.open {
            Some(open_key) if open_key == key && !t.first => {}
            Some(_) => return Err(WriteError::Misuse(Misuse::NotAContinuation)),
            None if !t.first => return Err(WriteError::Misuse(Misuse::NotAContinuation)),
            None => {
                let comma = if key {
                    self.key_slot()
                } else {
                    self.value_slot()
                }?;
                self.write_all(out, &[if comma { b"," } else { b"" }, b"\""])?;
                self.open = Some(key);
            }
        }
        if let Err(e) = escaped(t.fragment, out) {
            self.failed = true;
            return Err(WriteError::Output(e));
        }
        if t.last {
            let close: &[u8] = if key {
                b"\":"
            } else if self.stack.is_empty() && self.framing != Framing::Single {
                b"\"\n"
            } else {
                b"\""
            };
            self.write_all(out, &[close])?;
            self.open = None;
            if key {
                self.after_key = true;
            } else {
                self.value_done();
            }
        }
        Ok(())
    }

    /// A scalar's bytes, after the comma if `comma` and then `sign`.
    fn scalar<O: Output>(
        &mut self,
        comma: bool,
        sign: &[u8],
        bytes: &[u8],
        out: &mut O,
    ) -> Done<O> {
        let line = self.stack.is_empty() && self.framing != Framing::Single;
        self.write_all(
            out,
            &[
                if comma { b"," } else { b"" },
                sign,
                bytes,
                if line { b"\n" } else { b"" },
            ],
        )?;
        self.value_done();
        Ok(())
    }

    /// A value ended in the innermost container, or at the top level.
    fn value_done(&mut self) {
        if self.stack.is_empty() {
            self.done = true;
        }
        self.need_comma = true;
        self.after_key = false;
    }

    /// Writes each non-empty part; a failed write marks the writer failed.
    fn write_all<O: Output>(&mut self, out: &mut O, parts: &[&[u8]]) -> Done<O> {
        for part in parts.iter().filter(|p| !p.is_empty()) {
            if let Err(e) = out.write(part) {
                self.failed = true;
                return Err(WriteError::Output(e));
            }
        }
        Ok(())
    }
}

impl fmt::Display for Misuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self {
            Misuse::KeyNotAllowed => "a key is not allowed here",
            Misuse::ValueNotAllowed => "a value is not allowed here",
            Misuse::EndNotAllowed => "this end does not close the open container",
            Misuse::StringOpen => "a string is open",
            Misuse::NotAContinuation => "this fragment does not continue the open string",
            Misuse::TooDeep => "nesting is deeper than the configured limit",
            Misuse::InvalidNumber => "not a JSON number",
            Misuse::Unfinished => "the output is not complete",
            Misuse::Failed => "an earlier write failed",
        };
        f.write_str(what)
    }
}

impl std::error::Error for Misuse {}

impl<E: fmt::Display> fmt::Display for WriteError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Misuse(m) => write!(f, "refused: {m}"),
            WriteError::Output(e) => write!(f, "output failed: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for WriteError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WriteError::Misuse(m) => Some(m),
            WriteError::Output(e) => Some(e),
        }
    }
}

impl<E> From<Misuse> for WriteError<E> {
    fn from(m: Misuse) -> Self {
        WriteError::Misuse(m)
    }
}

fn whole(fragment: &str) -> Text<'_> {
    Text {
        fragment,
        first: true,
        last: true,
    }
}
