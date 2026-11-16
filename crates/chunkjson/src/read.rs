//! A blocking adapter over [`std::io::Read`], reading into a caller-supplied buffer.

use std::fmt;
use std::io::{self, Read};

use crate::tokenizer::Out;
use crate::{Config, Error, Event, Tokenizer};

/// A byte buffer the reader fills. Implement it for a pooled buffer to keep
/// every read inside a memory budget; the whole slice is used.
pub trait Buffer {
    /// The buffer's bytes, to read into.
    fn bytes_mut(&mut self) -> &mut [u8];
    /// The buffer's bytes, to tokenize.
    fn bytes(&self) -> &[u8];
}

impl Buffer for Vec<u8> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl Buffer for Box<[u8]> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl<const N: usize> Buffer for [u8; N] {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl Buffer for &mut [u8] {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
    fn bytes(&self) -> &[u8] {
        self
    }
}

/// An error from [`JsonReader`]: the source failed, or the JSON did.
#[derive(Debug)]
pub enum ReadError {
    /// The underlying reader failed.
    Io(io::Error),
    /// The input is not valid JSON.
    Json(Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "read failed: {e}"),
            Self::Json(e) => write!(f, "invalid JSON: {e}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
        }
    }
}

impl From<Error> for ReadError {
    fn from(e: Error) -> Self {
        Self::Json(e)
    }
}

/// Pulls events from a [`Read`], one buffer-full at a time. Never returns
/// `NeedMore`; returns `Eof` once the source is exhausted and the input complete.
/// A zero-length buffer reads nothing, so the input looks empty.
#[derive(Debug)]
pub struct JsonReader<R, B> {
    reader: R,
    buf: B,
    start: usize,
    end: usize,
    eof: bool,
    tokenizer: Tokenizer,
}

impl<R: Read, B: Buffer> JsonReader<R, B> {
    /// Reads `reader` through `buf`.
    pub fn new(reader: R, buf: B, config: Config) -> Self {
        Self {
            reader,
            buf,
            start: 0,
            end: 0,
            eof: false,
            tokenizer: Tokenizer::new(config),
        }
    }

    /// The tokenizer, for its offset and depth.
    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// The next event.
    pub fn next_event(&mut self) -> Result<Event<'_>, ReadError> {
        loop {
            if self.start == self.end && !self.eof {
                self.fill()?;
            }
            let from = self.start;
            let window = self.buf.bytes().get(from..self.end).unwrap_or_default();
            let (out, used) = self.tokenizer.step(window, self.eof)?;
            self.start = from.saturating_add(used);
            if matches!(out, Out::Simple(Event::NeedMore)) {
                continue;
            }
            let window = self.buf.bytes().get(from..self.end).unwrap_or_default();
            return Ok(self.tokenizer.resolve(out, window, 0, None)?);
        }
    }

    fn fill(&mut self) -> Result<(), ReadError> {
        loop {
            match self.reader.read(self.buf.bytes_mut()) {
                Ok(0) => self.eof = true,
                Ok(n) => self.end = n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ReadError::Io(e)),
            }
            self.start = 0;
            if self.eof {
                self.end = 0;
            }
            return Ok(());
        }
    }
}
