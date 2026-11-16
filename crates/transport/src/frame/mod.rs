//! Transport frames: writing a request or response, and reading a whole
//! frame back (`NativeOutboundMessage`, `InboundDecoder`).

mod compress;
mod header;

pub use compress::{MARKER as COMPRESSED_MARKER, deflate, inflate_bounded};
pub use header::{
    FIXED_HEADER_BYTES, FixedHeader, MARKER, MAX_VARIABLE_HEADER_BYTES, PING, Prefix, Status,
    VariableHeader,
};

use std::borrow::Cow;

use crate::error::WireError;
use crate::stream::Output;
use crate::version::WireVersion;

/// What goes in a frame's fixed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    /// The request id.
    pub request_id: i64,
    /// The status bits; the compress bit decides whether the message is
    /// compressed.
    pub status: Status,
    /// The version written: this node's, or its minimum compatible one in a
    /// TCP handshake request.
    pub version: WireVersion,
}

/// Appends one frame to `buf`, at most `limit` bytes long: the fixed
/// header, `variable`, then the message `write_message` writes, compressed
/// when the status says so.
///
/// # Errors
///
/// [`WireError::TooLarge`] when the frame passes `limit` or its variable
/// header passes [`MAX_VARIABLE_HEADER_BYTES`], which no reader accepts; whatever
/// `write_message` returns. `buf` is left as it was on an error.
pub fn encode<F>(
    buf: &mut Vec<u8>,
    limit: usize,
    meta: FrameMeta,
    variable: &VariableHeader,
    write_message: F,
) -> Result<(), WireError>
where
    F: FnOnce(&mut Output<'_>) -> Result<(), WireError>,
{
    let start = buf.len();
    let result = encode_at(buf, start, limit, meta, variable, write_message);
    if result.is_err() {
        buf.truncate(start);
    }
    result
}

fn encode_at<F>(
    buf: &mut Vec<u8>,
    start: usize,
    limit: usize,
    meta: FrameMeta,
    variable: &VariableHeader,
    write_message: F,
) -> Result<(), WireError>
where
    F: FnOnce(&mut Output<'_>) -> Result<(), WireError>,
{
    let bound = start.saturating_add(limit);
    let mut out = Output::new(buf, bound);
    out.bytes(&[0u8; FIXED_HEADER_BYTES])?;
    variable.write(&mut out, meta.status.is_request())?;
    let message_start = out.len();
    let variable_bytes = message_start.saturating_sub(start.saturating_add(FIXED_HEADER_BYTES));
    write_message(&mut out)?;
    if meta.status.is_compressed() {
        let compressed = deflate(buf.get(message_start..).unwrap_or_default());
        buf.truncate(message_start);
        Output::new(buf, bound).bytes(&compressed)?;
    }
    let after_prefix = buf.len().saturating_sub(start.saturating_add(6));
    let length = i32::try_from(after_prefix)
        .map_err(|_| WireError::too_large("a frame", i32::MAX as usize, after_prefix))?;
    if variable_bytes > MAX_VARIABLE_HEADER_BYTES {
        return Err(WireError::too_large(
            "a variable header",
            MAX_VARIABLE_HEADER_BYTES,
            variable_bytes,
        ));
    }
    // Within 64 KiB, so it fits an int.
    let variable_len = i32::try_from(variable_bytes).unwrap_or(i32::MAX);
    let mut fixed = [0u8; FIXED_HEADER_BYTES];
    {
        // ALLOC: none; writes into the fixed array through a bounded writer.
        let mut scratch = Vec::with_capacity(FIXED_HEADER_BYTES);
        let mut header = Output::new(&mut scratch, FIXED_HEADER_BYTES);
        header.bytes(&MARKER)?;
        header.int(length)?;
        header.long(meta.request_id)?;
        header.byte(meta.status.0)?;
        header.int(i32::from_ne_bytes(meta.version.wire_id().to_ne_bytes()))?;
        header.int(variable_len)?;
        fixed.copy_from_slice(&scratch);
    }
    if let Some(slot) = buf.get_mut(start..start.saturating_add(FIXED_HEADER_BYTES)) {
        slot.copy_from_slice(&fixed);
    }
    Ok(())
}

/// A whole frame, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<'a> {
    /// The fixed header.
    pub header: FixedHeader,
    /// The variable header.
    pub variable: VariableHeader,
    /// The message, inflated when it came compressed.
    pub message: Cow<'a, [u8]>,
}

/// Reads one whole frame (not a ping), refusing one past `max_frame_bytes`
/// or whose message inflates past it.
///
/// # Errors
///
/// As [`FixedHeader::read`], [`VariableHeader::read`] and
/// [`inflate_bounded`]; [`WireError::Malformed`] when `bytes` is not exactly
/// the frame its length announces.
pub fn decode(bytes: &[u8], max_frame_bytes: usize) -> Result<Frame<'_>, WireError> {
    const WHAT: &str = "a frame";
    let mut fixed = [0u8; FIXED_HEADER_BYTES];
    fixed.copy_from_slice(
        bytes
            .get(..FIXED_HEADER_BYTES)
            .ok_or(WireError::Truncated { what: WHAT })?,
    );
    let header = FixedHeader::read(&fixed, max_frame_bytes)?;
    let variable_end = FIXED_HEADER_BYTES.saturating_add(header.variable_header_bytes);
    let end = variable_end.saturating_add(header.message_bytes);
    if bytes.len() != end {
        return Err(WireError::malformed(
            WHAT,
            "the bytes are not the frame its length announces",
        ));
    }
    let variable = VariableHeader::read(
        bytes
            .get(FIXED_HEADER_BYTES..variable_end)
            .unwrap_or_default(),
        header.status.is_request(),
    )?;
    let raw = bytes.get(variable_end..).unwrap_or_default();
    let message = if header.status.is_compressed() {
        Cow::Owned(inflate_bounded(raw, max_frame_bytes)?)
    } else {
        Cow::Borrowed(raw)
    };
    Ok(Frame {
        header,
        variable,
        message,
    })
}
