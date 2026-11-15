//! Request bodies, streamed in bounded chunks.
//!
//! A request with a body takes one fixed-size buffer from the node's
//! request-body pool before a byte of the body is read; the body is then
//! read into that buffer one chunk at a time and handed to the handler.
//! Nothing holds the whole body. A handler that keeps bytes past a chunk (a
//! parsed query, documents waiting for the write log) reserves them first
//! through [`Body::reserve`], from the same pool.
//!
//! Framing is `Content-Length` or `Transfer-Encoding: chunked`. A declared
//! length over `http.max_content_length` is refused with 413 before reading;
//! a chunked body is refused with 413 the moment it passes the limit.

use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_mem::{BufferPool, Exhausted, MemoryBudget, Pool, PooledBuffer, Reservation};

use crate::error::RestError;
use crate::inbound::Source;

/// The label a refused body carries in its `circuit_breaking_exception`,
/// as OpenSearch's in-flight requests breaker labels it.
pub(crate) const BREAKER_LABEL: &str = "<http_request>";

/// The node's body buffers, with a bounded wait for one to come free.
#[derive(Debug)]
pub(crate) struct BodyBuffers {
    pool: BufferPool,
    lock: Mutex<()>,
    freed: Condvar,
}

/// A body buffer, returned to its pool (and a waiter woken) on drop.
#[derive(Debug)]
pub(crate) struct BodyBuffer {
    buf: Option<PooledBuffer>,
    owner: Arc<BodyBuffers>,
}

impl BodyBuffers {
    /// `count` buffers of `size` bytes, charged to the request-body pool.
    pub(crate) fn new(budget: &MemoryBudget, size: usize, count: usize) -> Result<Self, Exhausted> {
        Ok(Self {
            pool: BufferPool::new(budget, Pool::RequestBodies, size, count)?,
            lock: Mutex::new(()),
            freed: Condvar::new(),
        })
    }

    /// Takes a free buffer, waiting up to `wait` for one; then `Exhausted`.
    pub(crate) fn checkout(self: &Arc<Self>, wait: Duration) -> Result<BodyBuffer, Exhausted> {
        let deadline = Instant::now().checked_add(wait);
        let mut guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match self.pool.try_checkout() {
                Ok(buf) => {
                    return Ok(BodyBuffer {
                        buf: Some(buf),
                        owner: Arc::clone(self),
                    });
                }
                Err(e) => {
                    let left =
                        deadline.map_or(wait, |d| d.saturating_duration_since(Instant::now()));
                    if left.is_zero() {
                        return Err(e);
                    }
                    guard = self
                        .freed
                        .wait_timeout(guard, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }

    /// How many buffers are free.
    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.pool.available()
    }
}

impl Drop for BodyBuffer {
    fn drop(&mut self) {
        drop(self.buf.take());
        let _guard = self
            .owner
            .lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.owner.freed.notify_one();
    }
}

impl BodyBuffer {
    fn bytes(&mut self) -> &mut [u8] {
        self.buf.as_deref_mut().unwrap_or_default()
    }
}

/// How the body is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    /// No body.
    Empty,
    /// `Content-Length` bytes remain.
    Length(u64),
    /// Chunked; the state of the chunk being read.
    Chunked(Chunk),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chunk {
    /// Expecting a chunk-size line.
    Size,
    /// This many data bytes of the current chunk remain.
    Data(u64),
    /// Expecting the line ending after a chunk's data.
    DataEnd,
}

/// The longest chunk-size line (size and extensions) accepted.
const MAX_CHUNK_LINE: usize = 1024;

/// A request body, read in chunks into its buffer.
pub struct Body<'a> {
    src: &'a mut dyn Source,
    framing: Framing,
    buffer: Option<BodyBuffer>,
    read: u64,
    max: u64,
    max_trailer: usize,
    budget: &'a MemoryBudget,
    retained: Option<Reservation>,
}

impl std::fmt::Debug for Body<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body")
            .field("framing", &self.framing)
            .field("read", &self.read)
            .finish_non_exhaustive()
    }
}

impl<'a> Body<'a> {
    pub(crate) fn new(
        src: &'a mut dyn Source,
        framing: Framing,
        buffer: Option<BodyBuffer>,
        max: u64,
        max_trailer: usize,
        budget: &'a MemoryBudget,
    ) -> Self {
        Self {
            src,
            framing,
            buffer,
            read: 0,
            max,
            max_trailer,
            budget,
            retained: None,
        }
    }

    /// The declared `Content-Length`, or `None` for a chunked body. A
    /// request without a body has length zero.
    pub fn declared_length(&self) -> Option<u64> {
        match self.framing {
            Framing::Empty => Some(0),
            Framing::Length(n) => Some(n.saturating_add(self.read)),
            Framing::Chunked(_) => None,
        }
    }

    /// Whether the body has been read to its end.
    pub fn is_done(&self) -> bool {
        self.framing == Framing::Empty
    }

    /// Bytes of body read so far.
    pub fn bytes_read(&self) -> u64 {
        self.read
    }

    /// Reserves `bytes` more from the request-body pool for data the handler
    /// keeps past the current chunk; released when the request ends. Refused
    /// with 429 when the pool cannot hold them.
    pub fn reserve(&mut self, bytes: usize) -> Result<(), RestError> {
        let result = match &mut self.retained {
            Some(r) => r.try_grow(bytes),
            None => self
                .budget
                .try_reserve(Pool::RequestBodies, bytes)
                .map(|r| self.retained = Some(r)),
        };
        result.map_err(|e| RestError::circuit_breaking(&e, BREAKER_LABEL))
    }

    /// Bytes reserved through [`Body::reserve`].
    pub fn reserved(&self) -> usize {
        self.retained.as_ref().map_or(0, Reservation::bytes)
    }

    /// Reads and drops what is left of the body, up to `limit` bytes, so the
    /// connection can serve its next request; whether the body ended.
    pub(crate) fn drain(&mut self, limit: u64) -> bool {
        let start = self.read;
        while self.read.saturating_sub(start) <= limit {
            match self.next_chunk() {
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(_) => return false,
            }
        }
        false
    }

    /// The next chunk of the body, at most one buffer long, or `None` at its
    /// end. A short or malformed body, or a read timeout, closes the
    /// connection.
    pub fn next_chunk(&mut self) -> Result<Option<&[u8]>, RestError> {
        loop {
            match self.framing {
                Framing::Empty => return Ok(None),
                Framing::Length(0) => {
                    self.framing = Framing::Empty;
                    return Ok(None);
                }
                Framing::Length(left) => {
                    let n = self.read_data(left)?;
                    self.framing = Framing::Length(left.saturating_sub(as_u64(n)));
                    return Ok(Some(self.filled(n)));
                }
                Framing::Chunked(Chunk::Size) => self.chunk_size()?,
                Framing::Chunked(Chunk::Data(left)) => {
                    let n = self.read_data(left)?;
                    let rest = left.saturating_sub(as_u64(n));
                    self.framing = Framing::Chunked(if rest == 0 {
                        Chunk::DataEnd
                    } else {
                        Chunk::Data(rest)
                    });
                    return Ok(Some(self.filled(n)));
                }
                Framing::Chunked(Chunk::DataEnd) => {
                    if self.line(2)? != 0 {
                        return Err(malformed("chunk data is not followed by a line ending"));
                    }
                    self.consume_line();
                    self.framing = Framing::Chunked(Chunk::Size);
                }
            }
        }
    }

    fn filled(&mut self, n: usize) -> &[u8] {
        let buf = match &mut self.buffer {
            Some(b) => b.bytes(),
            None => &mut [],
        };
        &buf[..n.min(buf.len())]
    }

    /// Reads up to `left` body bytes into the buffer.
    fn read_data(&mut self, left: u64) -> Result<usize, RestError> {
        let Some(buffer) = &mut self.buffer else {
            return Err(malformed("a body arrived without a body buffer"));
        };
        let buf = buffer.bytes();
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        let n = self
            .src
            .read_into(&mut buf[..want])
            .map_err(RestError::from)?;
        if n == 0 {
            return Err(RestError::from(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the connection closed inside the request body",
            )));
        }
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        self.read = self.read.saturating_add(n64);
        Ok(n)
    }

    /// Reads one line into the source buffer and returns its length without
    /// the line ending, consuming it; a line longer than `max` is malformed.
    fn line(&mut self, max: usize) -> Result<usize, RestError> {
        loop {
            let buffered = self.src.buffered();
            if let Some(nl) = buffered.iter().position(|&b| b == b'\n') {
                let len = if nl > 0 && buffered[nl.saturating_sub(1)] == b'\r' {
                    nl.saturating_sub(1)
                } else {
                    nl
                };
                if len > max {
                    return Err(malformed("chunk line too long"));
                }
                return Ok(len);
            }
            if buffered.len() > max.saturating_add(1) {
                return Err(malformed("chunk line too long"));
            }
            match self.src.fill() {
                Ok(0) => {
                    return Err(RestError::from(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the connection closed inside a chunked body",
                    )));
                }
                Ok(_) => {}
                Err(e) => return Err(RestError::from(e)),
            }
        }
    }

    /// Consumes the line `line` measured, with its line ending.
    fn consume_line(&mut self) {
        let nl = self
            .src
            .buffered()
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(0);
        self.src.consume(nl.saturating_add(1));
    }

    fn chunk_size(&mut self) -> Result<(), RestError> {
        let len = self.line(MAX_CHUNK_LINE)?;
        let line = &self.src.buffered()[..len];
        let digits = line.split(|&b| b == b';').next().unwrap_or_default();
        let digits = digits.trim_ascii();
        if digits.is_empty() || digits.len() > 16 {
            return Err(malformed("invalid chunk size"));
        }
        let mut size: u64 = 0;
        for &d in digits {
            let v = char::from(d)
                .to_digit(16)
                .ok_or_else(|| malformed("invalid chunk size"))?;
            // At most 16 hex digits: the shift never drops a set bit.
            size = size.wrapping_shl(4) | u64::from(v);
        }
        self.consume_line();
        if size == 0 {
            self.trailers()?;
            self.framing = Framing::Empty;
            return Ok(());
        }
        if self.read.saturating_add(size) > self.max {
            return Err(RestError::too_large());
        }
        self.framing = Framing::Chunked(Chunk::Data(size));
        Ok(())
    }

    /// Reads and drops the trailer section, bounded by the header limit.
    fn trailers(&mut self) -> Result<(), RestError> {
        let mut total: usize = 0;
        loop {
            let len = self.line(self.max_trailer)?;
            self.consume_line();
            if len == 0 {
                return Ok(());
            }
            total = total.saturating_add(len).saturating_add(2);
            if total > self.max_trailer {
                return Err(malformed("chunked trailer too long"));
            }
        }
    }
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn malformed(reason: &'static str) -> RestError {
    RestError::illegal_argument(reason).closing()
}

#[cfg(test)]
#[path = "body_tests.rs"]
mod tests;
