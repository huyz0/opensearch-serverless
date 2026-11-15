//! A connection's socket with a budget of blocked time.
//!
//! A worker serves one connection at a time, so a peer that sends or reads
//! slowly holds a thread. A per-read socket timeout does not bound that: one
//! byte just inside each timeout keeps it alive forever. Here the time spent
//! blocked in reads and writes since [`Timed::restart`] is summed, and a
//! request may block for `slow_grace` plus `1 / min_transfer_rate` seconds
//! per byte moved; past that every read and write fails with `TimedOut`.
//!
//! Only a peer that is not keeping up is charged. Time this side spends
//! between socket calls (a handler parsing or appending, a wait for a body
//! buffer, backpressure from the write log) is never counted: while the
//! server is not reading, the peer's bytes queue in the socket and the next
//! read returns at once.

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// The socket, with the blocked-time budget of the request being served.
#[derive(Debug)]
pub(crate) struct Timed<'s> {
    stream: &'s TcpStream,
    grace: Duration,
    min_rate: u64,
    blocked: Cell<Duration>,
    moved: Cell<u64>,
}

impl<'s> Timed<'s> {
    pub(crate) fn new(stream: &'s TcpStream, grace: Duration, min_rate: u64) -> Self {
        Self {
            stream,
            grace,
            min_rate: min_rate.max(1),
            blocked: Cell::new(Duration::ZERO),
            moved: Cell::new(0),
        }
    }

    /// Starts a new request's budget.
    pub(crate) fn restart(&self) {
        self.blocked.set(Duration::ZERO);
        self.moved.set(0);
    }

    /// The blocked time left, or `TimedOut` when it is spent.
    fn remaining(&self) -> io::Result<Duration> {
        let earned_ms = self
            .moved
            .get()
            .saturating_mul(1000)
            .checked_div(self.min_rate)
            .unwrap_or(u64::MAX);
        let allowed = self.grace.saturating_add(Duration::from_millis(earned_ms));
        match allowed.checked_sub(self.blocked.get()) {
            Some(left) if !left.is_zero() => Ok(left),
            _ => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the peer is transferring too slowly",
            )),
        }
    }

    /// Runs one blocking call under the budget and charges what it took.
    fn charge(
        &self,
        set: impl FnOnce(&TcpStream, Duration) -> io::Result<()>,
        op: impl FnOnce(&TcpStream) -> io::Result<usize>,
    ) -> io::Result<usize> {
        set(self.stream, self.remaining()?)?;
        let started = Instant::now();
        let result = op(self.stream);
        self.blocked
            .set(self.blocked.get().saturating_add(started.elapsed()));
        if let Ok(n) = result {
            let n = u64::try_from(n).unwrap_or(u64::MAX);
            self.moved.set(self.moved.get().saturating_add(n));
        }
        result
    }
}

impl Read for &Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.charge(|s, t| s.set_read_timeout(Some(t)), |mut s| s.read(buf))
    }
}

impl Write for &Timed<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.charge(|s, t| s.set_write_timeout(Some(t)), |mut s| s.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut s = self.stream;
        s.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").expect("binds");
        let a = TcpStream::connect(l.local_addr().expect("addr")).expect("connects");
        let (b, _) = l.accept().expect("accepts");
        (a, b)
    }

    #[test]
    fn blocked_time_past_the_grace_fails_and_bytes_earn_more() {
        let (mut peer, server) = pair();
        let t = Timed::new(&server, Duration::from_millis(40), 1000);
        let mut buf = [0u8; 8];
        let e = (&t).read(&mut buf).expect_err("nothing sent");
        assert!(matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        // The budget is spent: the next read fails without blocking.
        let started = Instant::now();
        let e = (&t).read(&mut buf).expect_err("spent");
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(20));
        // 100 bytes at 1000 bytes a second earn 100 ms more.
        peer.write_all(&[1; 100]).expect("sends");
        t.moved.set(100);
        assert!(t.remaining().is_ok());
        t.restart();
        assert_eq!(t.remaining().ok(), Some(Duration::from_millis(40)));
        assert_eq!((&t).write(b"x").ok(), Some(1));
        assert!((&t).flush().is_ok());
    }

    #[test]
    fn a_peer_at_the_floor_keeps_its_connection_and_one_below_loses_it() {
        let (_peer, server) = pair();
        // The defaults: 8 KiB a second, 10 seconds of grace.
        let t = Timed::new(&server, Duration::from_secs(10), 8 << 10);
        // 60 s blocked while moving 60 s worth of bytes at the floor: still
        // the whole grace left.
        t.moved.set(60 * (8 << 10));
        t.blocked.set(Duration::from_secs(60));
        assert_eq!(t.remaining().ok(), Some(Duration::from_secs(10)));
        // At half the floor the peer falls behind by half a second each
        // second: after 19.99 s it is still on, at 20 s it is cut off.
        t.restart();
        t.moved.set(20 * (4 << 10));
        t.blocked.set(Duration::from_millis(19_990));
        assert!(t.remaining().is_ok());
        t.blocked.set(Duration::from_secs(20));
        assert_eq!(
            t.remaining().map_err(|e| e.kind()),
            Err(io::ErrorKind::TimedOut)
        );
        // Stalled outright, it is cut off after the grace and not before.
        t.restart();
        t.blocked.set(Duration::from_millis(9_999));
        assert!(t.remaining().is_ok());
        t.blocked.set(Duration::from_secs(10));
        assert!(t.remaining().is_err());
    }
}
