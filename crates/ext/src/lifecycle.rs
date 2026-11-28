//! The `lifecycle` point's bytes: one event in, nothing out
//! (docs/specs/plugin-system AC-5, AC-17).
//!
//! Little-endian, length-prefixed, version byte first, the same rules as the
//! `rest` point: every length is checked against a ceiling before anything is
//! allocated, and a decode refuses trailing bytes.

use crate::rest::CodecError;

const VERSION: u8 = 1;
/// The most bytes of the event's kind, tenant or index.
pub const MAX_PART_BYTES: usize = 1024;

/// One thing that happened on the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The kind, one of the manifest's event names.
    pub kind: String,
    /// The tenant it happened to, empty when it is the node's.
    pub tenant: String,
    /// The index it happened to, empty when it is the node's.
    pub index: String,
    /// When, in milliseconds since the epoch.
    pub at_ms: u64,
}

fn put(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&u16::try_from(s.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], CodecError> {
    if bytes.len() < n {
        return Err(CodecError::Short);
    }
    let (head, rest) = bytes.split_at(n);
    *bytes = rest;
    Ok(head)
}

fn part(bytes: &mut &[u8]) -> Result<String, CodecError> {
    let b = take(bytes, 2)?;
    let n = usize::from(u16::from_le_bytes([
        b.first().copied().unwrap_or(0),
        b.get(1).copied().unwrap_or(0),
    ]));
    if n > MAX_PART_BYTES {
        return Err(CodecError::Bound);
    }
    let raw = take(bytes, n)?;
    // ALLOC: at most MAX_PART_BYTES, checked above.
    std::str::from_utf8(raw)
        .map(str::to_owned)
        .map_err(|_| CodecError::Text)
}

impl Event {
    /// The event's bytes.
    pub fn encode(&self) -> Vec<u8> {
        // ALLOC: one event, three short parts and a timestamp.
        let mut out = vec![VERSION];
        put(&mut out, &self.kind);
        put(&mut out, &self.tenant);
        put(&mut out, &self.index);
        out.extend_from_slice(&self.at_ms.to_le_bytes());
        out
    }

    /// An event from bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut i = bytes;
        if take(&mut i, 1)?.first().copied() != Some(VERSION) {
            return Err(CodecError::Version);
        }
        let kind = part(&mut i)?;
        let tenant = part(&mut i)?;
        let index = part(&mut i)?;
        let mut at = [0_u8; 8];
        at.copy_from_slice(take(&mut i, 8)?);
        if !i.is_empty() {
            return Err(CodecError::Trailing);
        }
        Ok(Self {
            kind,
            tenant,
            index,
            at_ms: u64::from_le_bytes(at),
        })
    }
}
