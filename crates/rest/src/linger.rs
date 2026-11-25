//! The graceful close of a connection: input already in flight is read and
//! discarded so it cannot turn the close into a reset.

use std::io::Read;
use std::net::TcpStream;
use std::time::Instant;

/// Request bytes a closing connection still reads and discards after its
/// last response, so the client receives that response before the socket
/// closes. Closing with unread input makes the TCP stack send a reset, and
/// on Windows a reset discards a response the client has not read yet.
const LINGER_LIMIT: u64 = 256 << 10;

/// How long a closing connection keeps discarding input in all.
const LINGER_TIME: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a closing connection waits for more input before it closes. A
/// client that sends nothing more costs the worker this long, not
/// [`LINGER_TIME`].
const LINGER_IDLE: std::time::Duration = std::time::Duration::from_millis(100);

/// The graceful close described on `serve` in `conn.rs`, returning how many bytes it
/// discarded. Uses a stack buffer, so it allocates nothing.
pub(crate) fn linger(stream: &TcpStream) -> u64 {
    if stream.shutdown(std::net::Shutdown::Write).is_err() {
        return 0;
    }
    let _ = stream.set_read_timeout(Some(LINGER_IDLE));
    let deadline = Instant::now().checked_add(LINGER_TIME);
    let mut sink = [0u8; 4096];
    let mut discarded = 0u64;
    let mut reader = stream;
    while deadline.is_some_and(|d| Instant::now() < d) {
        let room = usize::try_from(LINGER_LIMIT.saturating_sub(discarded))
            .unwrap_or(usize::MAX)
            .min(sink.len());
        if room == 0 {
            break;
        }
        match reader.read(&mut sink[..room]) {
            Ok(0) | Err(_) => break,
            Ok(n) => discarded = discarded.saturating_add(u64::try_from(n).unwrap_or(u64::MAX)),
        }
    }
    discarded
}

#[cfg(test)]
#[path = "conn_linger_tests.rs"]
mod tests;
