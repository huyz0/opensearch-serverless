//! Reading frames off a socket into reserved memory.
//!
//! A frame is read in three steps, each bounded before its bytes are read:
//! the 6-byte prefix (refused past the frame bound), the rest of the fixed
//! header and the variable header (at most 64 KiB, reserved), then, after
//! the caller's precheck of the headers (the authentication's cheap checks),
//! the message (reserved). A frame that cannot be held, or that the precheck
//! refuses, is read through a fixed buffer and dropped, so the channel stays
//! in step and the sender gets an answer for its request id: closing one
//! channel would drop a Java peer's whole connection.

use std::io::{self, Read};

use oss_mem::{MemoryBudget, Pool, Reservation};

use crate::error::WireError;
use crate::exception::RemoteException;
use crate::frame::{FIXED_HEADER_BYTES, FixedHeader, Prefix, VariableHeader, inflate_bounded};

/// A frame held in reserved memory.
#[derive(Debug)]
pub struct HeldFrame {
    /// The fixed header.
    pub header: FixedHeader,
    /// The variable header.
    pub variable: VariableHeader,
    /// The message, inflated when it came compressed.
    pub message: Vec<u8>,
    _reservation: Reservation,
}

/// What one read off a channel gives.
#[derive(Debug)]
pub enum Inbound {
    /// A keep-alive ping.
    Ping,
    /// A whole frame.
    Frame(HeldFrame),
    /// A frame read and dropped: past the bound, or more than the pool
    /// holds, or refused by the precheck. The caller answers a request with
    /// `error` and fails a response's waiter with it.
    Dropped {
        /// The request id it carried.
        request_id: i64,
        /// Whether it was a request.
        is_request: bool,
        /// Why it was dropped.
        error: RemoteException,
    },
}

/// Why a channel cannot be read any more.
#[derive(Debug)]
pub enum ChannelError {
    /// The socket failed or timed out.
    Io(io::Error),
    /// The bytes are not the wire; nothing after them can be trusted.
    Wire(WireError),
}

impl From<io::Error> for ChannelError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WireError> for ChannelError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}

/// Reads frames from one channel, charging them to `Pool::Transport`.
#[derive(Debug, Clone)]
pub struct FrameReader {
    budget: MemoryBudget,
    max_frame_bytes: usize,
}

/// Bytes a dropped frame is read through at a time.
const DISCARD_CHUNK: usize = 8 * 1024;

impl FrameReader {
    /// A reader that holds frames of at most `max_frame_bytes`.
    pub fn new(budget: MemoryBudget, max_frame_bytes: usize) -> Self {
        Self {
            budget,
            max_frame_bytes,
        }
    }

    /// Reads the next frame, or `None` when the peer closed the channel
    /// between frames. `precheck` sees the headers before the message is
    /// reserved, and refuses a request by returning the error to answer.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Io`] when the socket fails, times out, or ends inside
    /// a frame; [`ChannelError::Wire`] for bytes that are not a frame.
    pub fn read<R, P>(&self, stream: &mut R, precheck: P) -> Result<Option<Inbound>, ChannelError>
    where
        R: Read,
        P: FnOnce(&FixedHeader, &VariableHeader) -> Result<(), RemoteException>,
    {
        let mut prefix = [0u8; 6];
        if !read_or_eof(stream, &mut prefix)? {
            return Ok(None);
        }
        // Past the bound, the frame's length still says how much to drop.
        let (rest, oversized) = match Prefix::read(&prefix, self.max_frame_bytes) {
            Ok(Prefix::Ping) => return Ok(Some(Inbound::Ping)),
            Ok(Prefix::Message { rest }) => (rest, false),
            Err(WireError::TooLarge { actual, .. }) => (
                usize::try_from(actual)
                    .unwrap_or(usize::MAX)
                    .saturating_sub(6),
                true,
            ),
            Err(e) => return Err(e.into()),
        };
        let mut fixed = [0u8; FIXED_HEADER_BYTES];
        if let Some(head) = fixed.get_mut(..6) {
            head.copy_from_slice(&prefix);
        }
        stream.read_exact(fixed.get_mut(6..).unwrap_or_default())?;
        let unread = rest.saturating_sub(FIXED_HEADER_BYTES - 6);
        if oversized {
            let mut input = crate::stream::Input::new(fixed.get(6..).unwrap_or_default());
            let request_id = input.long("a frame header")?;
            let status = crate::frame::Status(input.byte("a frame header")?);
            discard(stream, unread)?;
            return Ok(Some(Inbound::Dropped {
                request_id,
                is_request: status.is_request(),
                error: too_large(self.max_frame_bytes),
            }));
        }
        let header = FixedHeader::read(&fixed, self.max_frame_bytes)?;
        let dropped = |error| Inbound::Dropped {
            request_id: header.request_id,
            is_request: header.status.is_request(),
            error,
        };
        let Ok(mut reservation) = self
            .budget
            .try_reserve(Pool::Transport, header.variable_header_bytes)
        else {
            discard(stream, unread)?;
            return Ok(Some(dropped(busy())));
        };
        // ALLOC: the variable header, at most 64 KiB, reserved above.
        let mut variable_bytes = vec![0u8; header.variable_header_bytes];
        stream.read_exact(&mut variable_bytes)?;
        let variable = VariableHeader::read(&variable_bytes, header.status.is_request())?;
        drop(variable_bytes);
        if let Err(error) = precheck(&header, &variable) {
            discard(stream, header.message_bytes)?;
            return Ok(Some(dropped(error)));
        }
        reservation.shrink(header.variable_header_bytes);
        if reservation.try_grow(header.message_bytes).is_err() {
            discard(stream, header.message_bytes)?;
            return Ok(Some(dropped(busy())));
        }
        // ALLOC: the message, at most the frame bound, reserved above.
        let mut message = vec![0u8; header.message_bytes];
        stream.read_exact(&mut message)?;
        if header.status.is_compressed() {
            // Deflate declares no inflated length: reserve what the pool can
            // spare up to the bound, inflate within it, and give back the rest.
            // The compressed bytes stay held while the inflated ones grow.
            let room = self.max_frame_bytes;
            let spare = room.min(self.budget.available(Pool::Transport));
            if reservation.try_grow(spare).is_err() {
                return Ok(Some(dropped(busy())));
            }
            match inflate_bounded(&message, spare) {
                Ok(inflated) => {
                    reservation.shrink(reservation.bytes().saturating_sub(inflated.len()));
                    message = inflated;
                }
                Err(WireError::TooLarge { .. }) if spare < room => {
                    return Ok(Some(dropped(busy())));
                }
                Err(WireError::TooLarge { .. }) => {
                    return Ok(Some(dropped(too_large(self.max_frame_bytes))));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(Inbound::Frame(HeldFrame {
            header,
            variable,
            message,
            _reservation: reservation,
        })))
    }
}

/// The rejection a node answers when it cannot hold a frame now: 429, which
/// the sender may retry.
fn busy() -> RemoteException {
    RemoteException::rejected("transport memory is full; retry later")
}

/// The failure for a frame past the bound: 413, which a retry does not fix.
fn too_large(bound: usize) -> RemoteException {
    // ALLOC: one message per refused frame.
    RemoteException::status(
        &format!("a transport frame is larger than {bound} bytes"),
        "REQUEST_ENTITY_TOO_LARGE",
    )
}

/// Fills `buf`, or answers false when the stream ends before its first
/// byte.
fn read_or_eof<R: Read>(stream: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match stream.read(buf.get_mut(filled..).unwrap_or_default()) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled = filled.saturating_add(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Reads and drops `len` bytes through a fixed buffer.
fn discard<R: Read>(stream: &mut R, mut len: usize) -> io::Result<()> {
    let mut chunk = [0u8; DISCARD_CHUNK];
    while len > 0 {
        let take = len.min(DISCARD_CHUNK);
        stream.read_exact(chunk.get_mut(..take).unwrap_or_default())?;
        len = len.saturating_sub(take);
    }
    Ok(())
}
