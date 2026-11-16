//! The fixed and variable headers of a transport frame (`TcpHeader`,
//! `Header`, `NetworkMessage`).
//!
//! A frame is `E` `S`, a big-endian int counting the bytes after it, the
//! request id (long), a status byte, the sender's wire version (int), the
//! variable header's length (int), the variable header, then the message.
//! A length of -1 with nothing after it is a keep-alive ping.

use crate::error::WireError;
use crate::stream::{Input, Output};
use crate::version::WireVersion;

/// The marker every frame starts with.
pub const MARKER: [u8; 2] = *b"ES";

/// Bytes before the variable header: marker, length, request id, status,
/// version and variable-header length.
pub const FIXED_HEADER_BYTES: usize = 23;

/// Bytes a frame's length counts before its variable header: request id,
/// status, version and variable-header length.
const COUNTED_FIXED_BYTES: usize = FIXED_HEADER_BYTES - 6;

/// The longest variable header read: headers, features and action are a few
/// hundred bytes in practice.
pub const MAX_VARIABLE_HEADER_BYTES: usize = 64 * 1024;

/// A ping: the marker and a length of -1.
pub const PING: [u8; 6] = [b'E', b'S', 0xff, 0xff, 0xff, 0xff];

/// The status byte's bits (`TransportStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub u8);

impl Status {
    const RESPONSE: u8 = 1;
    const ERROR: u8 = 1 << 1;
    const COMPRESS: u8 = 1 << 2;
    const HANDSHAKE: u8 = 1 << 3;

    /// A request's status.
    pub fn request(compress: bool, handshake: bool) -> Self {
        Self(Self::bits(compress, handshake))
    }

    /// A response's status; `error` when it carries an exception.
    pub fn response(compress: bool, handshake: bool, error: bool) -> Self {
        let error = if error { Self::ERROR } else { 0 };
        Self(Self::RESPONSE | error | Self::bits(compress, handshake))
    }

    fn bits(compress: bool, handshake: bool) -> u8 {
        let compress = if compress { Self::COMPRESS } else { 0 };
        let handshake = if handshake { Self::HANDSHAKE } else { 0 };
        compress | handshake
    }

    /// Whether the frame is a request (its low bit clear).
    pub fn is_request(self) -> bool {
        self.0 & Self::RESPONSE == 0
    }

    /// Whether a response carries an exception.
    pub fn is_error(self) -> bool {
        self.0 & Self::ERROR != 0
    }

    /// Whether the message is compressed.
    pub fn is_compressed(self) -> bool {
        self.0 & Self::COMPRESS != 0
    }

    /// Whether the frame is part of the TCP handshake.
    pub fn is_handshake(self) -> bool {
        self.0 & Self::HANDSHAKE != 0
    }
}

/// What the first six bytes of a frame announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// A keep-alive ping; nothing follows.
    Ping,
    /// A message of `rest` more bytes after these six.
    Message {
        /// Bytes after the prefix: the rest of the fixed header, the
        /// variable header and the message.
        rest: usize,
    },
}

impl Prefix {
    /// Reads the marker and length, refusing a frame past `max_frame_bytes`
    /// (counting the whole frame) before a byte of it is read.
    ///
    /// # Errors
    ///
    /// [`WireError::Malformed`] for another marker or a length shorter than
    /// the fixed header; [`WireError::TooLarge`] past the bound.
    pub fn read(prefix: &[u8; 6], max_frame_bytes: usize) -> Result<Self, WireError> {
        const WHAT: &str = "a frame prefix";
        if prefix.get(..2) != Some(&MARKER[..]) {
            return Err(WireError::malformed(WHAT, "a frame starts with ES"));
        }
        let mut len = [0u8; 4];
        len.copy_from_slice(prefix.get(2..).unwrap_or_default());
        let len = i32::from_be_bytes(len);
        if len == -1 {
            return Ok(Self::Ping);
        }
        let rest = usize::try_from(len)
            .ok()
            .filter(|rest| *rest >= COUNTED_FIXED_BYTES)
            .ok_or(WireError::malformed(
                WHAT,
                "a frame is longer than its fixed header",
            ))?;
        let whole = rest.saturating_add(6);
        if whole > max_frame_bytes {
            return Err(WireError::too_large("a frame", max_frame_bytes, whole));
        }
        Ok(Self::Message { rest })
    }
}

/// A frame's fixed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedHeader {
    /// The request id a response answers.
    pub request_id: i64,
    /// The status bits.
    pub status: Status,
    /// The sender's version (its minimum compatible one in a TCP handshake
    /// request); checked by the caller.
    pub version: WireVersion,
    /// The variable header's length.
    pub variable_header_bytes: usize,
    /// The message's length (compressed, when compressed).
    pub message_bytes: usize,
}

impl FixedHeader {
    /// Reads the 23 fixed bytes, prefix included.
    ///
    /// # Errors
    ///
    /// As [`Prefix::read`]; a ping is [`WireError::Malformed`] here; a
    /// variable header longer than the frame or than
    /// [`MAX_VARIABLE_HEADER_BYTES`] is refused.
    pub fn read(
        bytes: &[u8; FIXED_HEADER_BYTES],
        max_frame_bytes: usize,
    ) -> Result<Self, WireError> {
        const WHAT: &str = "a frame header";
        let mut prefix = [0u8; 6];
        prefix.copy_from_slice(bytes.get(..6).unwrap_or_default());
        let Prefix::Message { rest } = Prefix::read(&prefix, max_frame_bytes)? else {
            return Err(WireError::malformed(WHAT, "a ping has no header"));
        };
        let mut input = Input::new(bytes.get(6..).unwrap_or_default());
        let request_id = input.long(WHAT)?;
        let status = Status(input.byte(WHAT)?);
        let version = WireVersion::from_wire_id(u32::from_ne_bytes(input.int(WHAT)?.to_ne_bytes()));
        let variable = usize::try_from(input.int(WHAT)?)
            .map_err(|_| WireError::malformed(WHAT, "a variable header length is not negative"))?;
        let content = rest.saturating_sub(COUNTED_FIXED_BYTES);
        if variable > MAX_VARIABLE_HEADER_BYTES {
            return Err(WireError::too_large(
                "a variable header",
                MAX_VARIABLE_HEADER_BYTES,
                variable,
            ));
        }
        let message_bytes = content.checked_sub(variable).ok_or(WireError::malformed(
            WHAT,
            "a variable header is longer than its frame",
        ))?;
        Ok(Self {
            request_id,
            status,
            version,
            variable_header_bytes: variable,
            message_bytes,
        })
    }
}

/// A frame's variable header: the sender's thread-context headers, and for
/// a request its features and action.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VariableHeader {
    /// Request headers, in wire order (the Java writes a hash map's order).
    pub request_headers: Vec<(String, String)>,
    /// Response headers and their values, in wire order.
    pub response_headers: Vec<(String, Vec<String>)>,
    /// The feature names a request announces.
    pub features: Vec<String>,
    /// A request's action; empty for a response.
    pub action: String,
}

impl VariableHeader {
    /// The request header named `key`, if present.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.request_headers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Writes the header of a request (`is_request`) or a response.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>, is_request: bool) -> Result<(), WireError> {
        out.string_map(&self.request_headers)?;
        out.count(self.response_headers.len())?;
        for (key, values) in &self.response_headers {
            out.string(key)?;
            out.count(values.len())?;
            for value in values {
                out.string(value)?;
            }
        }
        if is_request {
            out.count(self.features.len())?;
            for feature in &self.features {
                out.string(feature)?;
            }
            out.string(&self.action)?;
        }
        Ok(())
    }

    /// Reads the whole variable header of a request or a response.
    ///
    /// # Errors
    ///
    /// As the stream reads, and [`WireError::Malformed`] when bytes remain.
    pub fn read(bytes: &[u8], is_request: bool) -> Result<Self, WireError> {
        const WHAT: &str = "a variable header";
        let mut input = Input::new(bytes);
        let request_headers = input.string_map(WHAT)?;
        let count = input.count(2, WHAT)?;
        // ALLOC: at most one entry per two variable-header bytes, which are at most 64 KiB.
        let mut response_headers = Vec::with_capacity(count);
        for _ in 0..count {
            let key = input.string(WHAT)?;
            let values = input.string_array(WHAT)?;
            response_headers.push((key, values));
        }
        let (features, action) = if is_request {
            let features = input.string_array(WHAT)?;
            (features, input.string(WHAT)?)
        } else {
            // ALLOC: none; empty vector and string.
            (Vec::new(), String::new())
        };
        input.finish(WHAT)?;
        Ok(Self {
            request_headers,
            response_headers,
            features,
            action,
        })
    }
}
