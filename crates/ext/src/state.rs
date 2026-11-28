//! The bytes of the `state.*` host operations: a plugin's tenant-scoped
//! key-value state (docs/specs/plugin-system, Host API 1.0; ADR 0080,
//! section 5).
//!
//! Every message starts with the version byte 1, lengths are little-endian,
//! and a decode refuses a key or value over its ceiling before it allocates.

use crate::rest::CodecError;

const VERSION: u8 = 1;
/// The longest key, in bytes.
pub const MAX_KEY_BYTES: usize = 128;
/// The longest value, in bytes.
pub const MAX_VALUE_BYTES: usize = 256 * 1024;

/// A `state.put`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Put {
    /// The key.
    pub key: String,
    /// The value.
    pub value: Vec<u8>,
    /// Write only when the key holds nothing yet.
    pub if_absent: bool,
}

fn put_key(out: &mut Vec<u8>, key: &str) {
    out.extend_from_slice(&u16::try_from(key.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], CodecError> {
    if bytes.len() < n {
        return Err(CodecError::Short);
    }
    let (head, rest) = bytes.split_at(n);
    *bytes = rest;
    Ok(head)
}

fn version(bytes: &mut &[u8]) -> Result<(), CodecError> {
    match take(bytes, 1)?.first().copied() {
        Some(VERSION) => Ok(()),
        _ => Err(CodecError::Version),
    }
}

fn key(bytes: &mut &[u8]) -> Result<String, CodecError> {
    let len = take(bytes, 2)?;
    let n = usize::from(u16::from_le_bytes([
        len.first().copied().unwrap_or(0),
        len.get(1).copied().unwrap_or(0),
    ]));
    if n == 0 || n > MAX_KEY_BYTES {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_KEY_BYTES, checked above.
    std::str::from_utf8(take(bytes, n)?)
        .map(str::to_owned)
        .map_err(|_| CodecError::Text)
}

/// The bytes of a `state.get` or `state.delete` naming `key`.
pub fn encode_key(key_text: &str) -> Vec<u8> {
    // ALLOC: one call's bytes, at most a key.
    let mut out = vec![VERSION];
    put_key(&mut out, key_text);
    out
}

/// The key of a `state.get` or `state.delete`.
///
/// # Errors
///
/// Bytes that are not one key, or a key over its ceiling.
pub fn decode_key(bytes: &[u8]) -> Result<String, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let k = key(&mut i)?;
    if i.is_empty() {
        Ok(k)
    } else {
        Err(CodecError::Trailing)
    }
}

/// The bytes of a `state.put`.
pub fn encode_put(p: &Put) -> Vec<u8> {
    // ALLOC: one call's bytes, at most a key and a value.
    let mut out = vec![VERSION, u8::from(p.if_absent)];
    put_key(&mut out, &p.key);
    out.extend_from_slice(
        &u32::try_from(p.value.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&p.value);
    out
}

/// A `state.put` from bytes.
///
/// # Errors
///
/// Bytes that are not a put, or a key or value over its ceiling.
pub fn decode_put(bytes: &[u8]) -> Result<Put, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let if_absent = match take(&mut i, 1)?.first().copied() {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(CodecError::Value),
    };
    let k = key(&mut i)?;
    let len = take(&mut i, 4)?;
    let mut a = [0_u8; 4];
    a.copy_from_slice(len);
    let n = u32::from_le_bytes(a) as usize;
    if n > MAX_VALUE_BYTES {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_VALUE_BYTES, checked above.
    let value = take(&mut i, n)?.to_vec();
    if !i.is_empty() {
        return Err(CodecError::Trailing);
    }
    Ok(Put {
        key: k,
        value,
        if_absent,
    })
}

/// A `state.get` answer: nothing, or the value.
pub fn encode_found(value: Option<&[u8]>) -> Vec<u8> {
    match value {
        None => vec![0],
        Some(v) => {
            // ALLOC: one answer, at most a value.
            let mut out = Vec::with_capacity(v.len().saturating_add(1));
            out.push(1);
            out.extend_from_slice(v);
            out
        }
    }
}

/// A `state.get` answer from bytes.
///
/// # Errors
///
/// Bytes that are not an answer.
pub fn decode_found(bytes: &[u8]) -> Result<Option<Vec<u8>>, CodecError> {
    match bytes.split_first() {
        Some((0, [])) => Ok(None),
        Some((1, v)) => Ok(Some(v.to_vec())),
        Some(_) => Err(CodecError::Value),
        None => Err(CodecError::Short),
    }
}
