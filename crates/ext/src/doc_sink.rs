//! The seam a plugin writes documents through: `index.docs`, one call that
//! indexes a few documents into the node's own indexes by the path `_bulk`
//! takes (docs/specs/plugin-system, Host API 1.0).
//!
//! [`DocSink`] is the node's side, a plain trait over documents. The bytes a
//! plugin sends and receives are the codec below, so every tier calls the
//! same operation, and [`DocSinkBackend`] turns that operation into a
//! `DocSink` call and hands every other operation to the backend behind it.

use std::sync::Arc;

use crate::host::{Backend, HostError, Who};
use crate::rest::CodecError;

const VERSION: u8 = 1;
/// The most documents one call carries.
pub const MAX_DOCS: usize = 1000;
/// The most bytes of one document's source.
pub const MAX_SOURCE_BYTES: usize = 1 << 20;
/// The most bytes of an index name or a document id.
pub const MAX_NAME_BYTES: usize = 512;
/// The most bytes of all the sources of one call.
pub const MAX_CALL_BYTES: usize = 8 << 20;

/// One document to index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// Its id; none lets the node generate one.
    pub id: Option<String>,
    /// Its source, a JSON object.
    pub source: Vec<u8>,
}

/// What became of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocResult {
    /// Its id, generated when the plugin gave none.
    pub id: String,
    /// The HTTP status the item of a `_bulk` would carry.
    pub status: u16,
}

/// The node's side of `index.docs`.
pub trait DocSink: Send + Sync {
    /// Indexes `docs` into `index`, one result a document in order.
    ///
    /// # Errors
    ///
    /// Why the call as a whole was refused; a document that fails alone is a
    /// result with its status.
    fn index(&self, index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError>;
}

fn put_str(out: &mut Vec<u8>, s: &str) {
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

fn get_u16(bytes: &mut &[u8]) -> Result<u16, CodecError> {
    let b = take(bytes, 2)?;
    Ok(u16::from_le_bytes([
        b.first().copied().unwrap_or(0),
        b.get(1).copied().unwrap_or(0),
    ]))
}

fn get_str(bytes: &mut &[u8]) -> Result<String, CodecError> {
    let n = usize::from(get_u16(bytes)?);
    if n > MAX_NAME_BYTES {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_NAME_BYTES, checked above.
    std::str::from_utf8(take(bytes, n)?)
        .map(str::to_owned)
        .map_err(|_| CodecError::Text)
}

/// A call's bytes: the index and the documents.
pub fn encode_call(index: &str, docs: &[Doc]) -> Vec<u8> {
    // ALLOC: one call's bytes, bounded by the point's ceiling on the host.
    let mut out = vec![VERSION];
    put_str(&mut out, index);
    out.extend_from_slice(&u16::try_from(docs.len()).unwrap_or(u16::MAX).to_le_bytes());
    for d in docs {
        match &d.id {
            Some(id) => {
                out.push(1);
                put_str(&mut out, id);
            }
            None => out.push(0),
        }
        out.extend_from_slice(
            &u32::try_from(d.source.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(&d.source);
    }
    out
}

/// A call from bytes.
///
/// # Errors
///
/// Bytes that are not a call, or one over a ceiling.
pub fn decode_call(bytes: &[u8]) -> Result<(String, Vec<Doc>), CodecError> {
    let mut i = bytes;
    if take(&mut i, 1)?.first().copied() != Some(VERSION) {
        return Err(CodecError::Version);
    }
    let index = get_str(&mut i)?;
    let n = usize::from(get_u16(&mut i)?);
    if n > MAX_DOCS {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_DOCS documents, checked above.
    let mut docs = Vec::with_capacity(n);
    let mut total = 0_usize;
    for _ in 0..n {
        let id = match take(&mut i, 1)?.first().copied() {
            Some(0) => None,
            Some(1) => Some(get_str(&mut i)?),
            _ => return Err(CodecError::Value),
        };
        let len = take(&mut i, 4)?;
        let mut a = [0_u8; 4];
        a.copy_from_slice(len);
        let len = u32::from_le_bytes(a) as usize;
        total = total.saturating_add(len);
        if len > MAX_SOURCE_BYTES || total > MAX_CALL_BYTES {
            return Err(CodecError::Bound);
        }
        // ALLOC: at most MAX_SOURCE_BYTES, and MAX_CALL_BYTES in all, checked above.
        let source = take(&mut i, len)?.to_vec();
        docs.push(Doc { id, source });
    }
    if !i.is_empty() {
        return Err(CodecError::Trailing);
    }
    Ok((index, docs))
}

/// An answer's bytes.
pub fn encode_results(results: &[DocResult]) -> Vec<u8> {
    // ALLOC: one answer, at most MAX_DOCS entries.
    let mut out = vec![VERSION];
    out.extend_from_slice(
        &u16::try_from(results.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    for r in results {
        put_str(&mut out, &r.id);
        out.extend_from_slice(&r.status.to_le_bytes());
    }
    out
}

/// An answer from bytes.
///
/// # Errors
///
/// Bytes that are not an answer.
pub fn decode_results(bytes: &[u8]) -> Result<Vec<DocResult>, CodecError> {
    let mut i = bytes;
    if take(&mut i, 1)?.first().copied() != Some(VERSION) {
        return Err(CodecError::Version);
    }
    let n = usize::from(get_u16(&mut i)?);
    if n > MAX_DOCS {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_DOCS entries, checked above.
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let id = get_str(&mut i)?;
        let status = get_u16(&mut i)?;
        out.push(DocResult { id, status });
    }
    if !i.is_empty() {
        return Err(CodecError::Trailing);
    }
    Ok(out)
}

/// Serves `index.docs` from a [`DocSink`] and every other operation from
/// `rest`.
pub struct DocSinkBackend {
    sink: Arc<dyn DocSink>,
    rest: Arc<dyn Backend>,
}

impl DocSinkBackend {
    /// A backend writing through `sink`, deferring the rest to `rest`.
    pub fn new(sink: Arc<dyn DocSink>, rest: Arc<dyn Backend>) -> Self {
        Self { sink, rest }
    }
}

impl Backend for DocSinkBackend {
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        if op != "index.docs" {
            return self.rest.call(who, op, input);
        }
        let (index, docs) = decode_call(input).map_err(|_| HostError::Invalid)?;
        let results = self.sink.index(&index, &docs)?;
        Ok(encode_results(&results))
    }
}
