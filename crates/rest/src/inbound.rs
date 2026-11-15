//! The read side of a connection: a fixed buffer in front of the socket.
//!
//! Bytes read past the end of one request (a pipelined next request, or the
//! start of a body read together with its head) wait here. Body bytes beyond
//! what is buffered are read straight into the caller's body buffer, so a
//! body passes through this buffer only for chunk framing.

use std::io::{self, Read};

/// A connection's buffered read side.
pub(crate) trait Source {
    /// The buffered, unread bytes.
    fn buffered(&self) -> &[u8];
    /// Marks `n` buffered bytes as read.
    fn consume(&mut self, n: usize);
    /// Reads more into the buffer, moving unread bytes to its front first.
    /// `Ok(0)` is end of stream; a full buffer is an error.
    fn fill(&mut self) -> io::Result<usize>;
    /// Reads into `out`: buffered bytes first, then the socket.
    fn read_into(&mut self, out: &mut [u8]) -> io::Result<usize>;
}

/// A buffered reader over `R` with a fixed buffer.
#[derive(Debug)]
pub(crate) struct Inbound<R, B> {
    src: R,
    buf: B,
    start: usize,
    end: usize,
}

impl<R: Read, B: AsMut<[u8]> + AsRef<[u8]>> Inbound<R, B> {
    pub(crate) fn new(src: R, buf: B) -> Self {
        Self {
            src,
            buf,
            start: 0,
            end: 0,
        }
    }

    /// Takes the buffered bytes into `out`, as many as fit, and returns how
    /// many were moved.
    pub(crate) fn take_buffered(&mut self, out: &mut [u8]) -> usize {
        let n = self.buffered().len().min(out.len());
        out[..n].copy_from_slice(&self.buffered()[..n]);
        self.consume(n);
        n
    }

    /// Puts `bytes` back in front of the buffered ones: what a head read
    /// past its end. They must fit in the space already free.
    pub(crate) fn unread(&mut self, bytes: &[u8]) -> io::Result<()> {
        let pending = self.end.saturating_sub(self.start);
        let total = pending.saturating_add(bytes.len());
        if total > self.buf.as_ref().len() {
            return Err(io::Error::other("unread past the buffer"));
        }
        let buf = self.buf.as_mut();
        buf.copy_within(self.start..self.end, bytes.len());
        buf[..bytes.len()].copy_from_slice(bytes);
        self.start = 0;
        self.end = total;
        Ok(())
    }

    /// The underlying reader.
    #[cfg(test)]
    pub(crate) fn source(&self) -> &R {
        &self.src
    }
}

impl<R: Read, B: AsMut<[u8]> + AsRef<[u8]>> Source for Inbound<R, B> {
    fn buffered(&self) -> &[u8] {
        &self.buf.as_ref()[self.start..self.end]
    }

    fn consume(&mut self, n: usize) {
        self.start = self.start.saturating_add(n).min(self.end);
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
    }

    fn fill(&mut self) -> io::Result<usize> {
        if self.start > 0 {
            let (start, end) = (self.start, self.end);
            self.buf.as_mut().copy_within(start..end, 0);
            self.end = end.saturating_sub(start);
            self.start = 0;
        }
        let end = self.end;
        let free = &mut self.buf.as_mut()[end..];
        if free.is_empty() {
            return Err(io::Error::other("read buffer full"));
        }
        let n = self.src.read(free)?;
        self.end = end.saturating_add(n);
        Ok(n)
    }

    fn read_into(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.buffered().is_empty() {
            return Ok(self.take_buffered(out));
        }
        self.src.read(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_bytes_come_first_then_the_source() {
        let mut inb = Inbound::new(&b"abcdef"[..], [0u8; 4]);
        assert_eq!(inb.fill().ok(), Some(4));
        assert_eq!(inb.buffered(), b"abcd");
        inb.consume(1);
        let mut out = [0u8; 8];
        assert_eq!(inb.read_into(&mut out).ok(), Some(3));
        assert_eq!(&out[..3], b"bcd");
        assert_eq!(inb.read_into(&mut out).ok(), Some(2));
        assert_eq!(&out[..2], b"ef");
        assert_eq!(inb.read_into(&mut out).ok(), Some(0));
        assert!(inb.source().is_empty());
    }

    #[test]
    fn fill_compacts_and_a_full_buffer_is_an_error() {
        let mut inb = Inbound::new(&b"abcdefgh"[..], [0u8; 4]);
        assert_eq!(inb.fill().ok(), Some(4));
        assert!(inb.fill().is_err(), "full");
        inb.consume(2);
        assert_eq!(inb.fill().ok(), Some(2));
        assert_eq!(inb.buffered(), b"cdef");
        inb.consume(10);
        assert!(inb.buffered().is_empty());
    }

    #[test]
    fn unread_puts_bytes_in_front_up_to_the_buffer_size() {
        let mut inb = Inbound::new(&b"yz"[..], [0u8; 4]);
        assert_eq!(inb.fill().ok(), Some(2));
        inb.unread(b"wx").expect("fits");
        assert_eq!(inb.buffered(), b"wxyz");
        assert!(inb.unread(b"v").is_err(), "one past the buffer");
        let mut out = [0u8; 2];
        assert_eq!(inb.take_buffered(&mut out), 2);
        assert_eq!(&out, b"wx");
    }
}
