//! One socket's traffic after its handshakes: sending a request and waiting
//! for its answer, routing answers to their waiters, and telling failures
//! apart.

use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::PoisonError;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::{CallError, Connection};
use crate::channel::{ChannelError, FrameReader, HeldFrame, Inbound};
use crate::exception::{ExceptionKind, RemoteException, ids};

pub(super) fn send_and_wait(
    connection: &Connection,
    frame: &[u8],
    answer: &Receiver<Result<HeldFrame, CallError>>,
    deadline: Instant,
) -> Result<HeldFrame, CallError> {
    {
        let mut writer = connection
            .writer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        writer
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        if let Err(e) = writer.write_all(frame).and_then(|()| writer.flush()) {
            let _ = writer.shutdown(std::net::Shutdown::Both);
            return Err(io_error(e));
        }
    }
    match answer.recv_timeout(remaining(deadline)?) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(CallError::Timeout),
        Err(RecvTimeoutError::Disconnected) => Err(CallError::Io(io::ErrorKind::ConnectionAborted)),
    }
}

/// Routes a socket's answers to their waiters until it closes.
pub(super) fn route(mut stream: TcpStream, reader: &FrameReader, connection: &Connection) {
    loop {
        match reader.read(&mut stream, |_, _| Ok(())) {
            Ok(Some(Inbound::Frame(frame))) => {
                let waiter = connection
                    .waiters
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&frame.header.request_id);
                if let Some(waiter) = waiter {
                    let _ = waiter.try_send(Ok(frame));
                }
            }
            Ok(Some(Inbound::Dropped {
                request_id, error, ..
            })) => {
                let waiter = connection
                    .waiters
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&request_id);
                if let Some(waiter) = waiter {
                    let _ = waiter.try_send(Err(dropped(&error)));
                }
            }
            Ok(Some(Inbound::Ping)) => {}
            Ok(None) => return connection.fail_all(io::ErrorKind::UnexpectedEof),
            Err(ChannelError::Io(e)) => return connection.fail_all(e.kind()),
            Err(ChannelError::Wire(_)) => return connection.fail_all(io::ErrorKind::InvalidData),
        }
    }
}

/// Why this node could not hold an answer: past the frame bound, or more
/// than its transport memory has free now.
pub(super) fn dropped(error: &RemoteException) -> CallError {
    if error.http_status() == 429 {
        CallError::Busy
    } else {
        CallError::TooLarge
    }
}

/// A failure the peer answered, told apart: authentication refused, or any
/// other.
pub(super) fn classify(failure: RemoteException) -> CallError {
    let root = failure.root();
    let unauthorized = root.kind == Some(ExceptionKind::OpenSearch(ids::SECURITY))
        && matches!(failure.http_status(), 401 | 403);
    if unauthorized {
        CallError::Unauthorized(failure)
    } else {
        CallError::Remote(failure)
    }
}

pub(super) fn remaining(deadline: Instant) -> Result<Duration, CallError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(CallError::Timeout)
    } else {
        Ok(left)
    }
}

/// A connect's failure: refused only when the host answered that nothing
/// listens; an unreachable host or network is a timeout, as a partition is.
pub(super) fn connect_error(e: io::Error) -> CallError {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => CallError::Refused,
        io::ErrorKind::TimedOut
        | io::ErrorKind::WouldBlock
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable => CallError::Timeout,
        kind => CallError::Io(kind),
    }
}

pub(super) fn io_error(e: io::Error) -> CallError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => CallError::Timeout,
        kind => CallError::Io(kind),
    }
}
