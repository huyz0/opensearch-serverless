//! The `analyzer` point's wire form
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)): a
//! block of texts in, the tokens of each text out.
//!
//! The module analyses each text alone and the host joins the texts of a
//! multi-valued field with the position and offset gaps, so a module never
//! sees a gap. Input is little endian: a `u32` count, then per text a `u32`
//! byte length and its UTF-8 bytes. Output is per text a `u32` token count,
//! then per token a `u32` term length, the term bytes, a `u32` start, a `u32`
//! end (byte offsets into that text) and a `u32` position increment. The
//! host validates every output and converts offsets to the UTF-16 units
//! Lucene's offsets use.

use std::ops::Range;

use crate::error::CallError;
use crate::status::Status;

/// The most texts one call carries.
pub const MAX_BLOCK_TEXTS: usize = 64;
/// The most text bytes one call carries (the sum over its texts).
pub const MAX_BLOCK_TEXT_BYTES: usize = 1024 * 1024;
/// The most bytes of output one call may return.
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// The most tokens one call may return.
pub const MAX_TOKENS: usize = 100_000;
/// The longest term, Lucene's immense-term bound.
pub const MAX_TERM_BYTES: usize = 32_766;
/// The largest position a token may sit at.
const MAX_POSITION: i64 = 0x7FFF_FFFE;
/// The bytes of one token apart from its term.
const TOKEN_FIXED: usize = 16;

/// One token of a text, with offsets in UTF-16 units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The term.
    pub term: String,
    /// The first UTF-16 unit of the token in the text.
    pub start: u32,
    /// One past the last UTF-16 unit of the token in the text.
    pub end: u32,
    /// How far the token's position is past the one before it (past -1 for
    /// the first).
    pub increment: u32,
}

/// What a pinned analyzer is to a caller that indexes: one text in, its
/// checked tokens out. The host's call implements it over a module; a
/// test over a table.
pub trait Analyze: Send + Sync {
    /// The tokens of `text`.
    ///
    /// # Errors
    ///
    /// The call's typed failure, which the caller answers rather than
    /// falls back to a built-in analyzer.
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError>;

    /// The most time one document's calls to this analyzer may take in all,
    /// or `None` for no cap (a replay, a recovery, a test table).
    fn item_budget(&self) -> Option<std::time::Duration> {
        None
    }
}

/// How many deadlines one document's calls to a module may take in all.
pub const ITEM_DEADLINES: u32 = 10;

fn limit(why: &'static [u8]) -> CallError {
    CallError::from_status(Status::Limit, why)
}

fn bad(why: &'static str) -> CallError {
    CallError::malformed_because(why)
}

fn size(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Whether `texts` fit one call.
fn check(texts: &[&str]) -> Result<(), CallError> {
    if texts.is_empty() {
        return Err(limit(b"a block of no texts"));
    }
    if texts.len() > MAX_BLOCK_TEXTS {
        return Err(limit(b"more than 64 texts in a block"));
    }
    let bytes = texts.iter().fold(0_usize, |a, t| a.saturating_add(t.len()));
    if bytes > MAX_BLOCK_TEXT_BYTES {
        return Err(limit(b"more than 1 MiB of text in a block"));
    }
    Ok(())
}

/// The input of one call for `texts`.
///
/// # Errors
///
/// A limit failure when the block has no text, more than
/// [`MAX_BLOCK_TEXTS`] texts or more than [`MAX_BLOCK_TEXT_BYTES`] bytes.
pub fn encode_block(texts: &[&str]) -> Result<Vec<u8>, CallError> {
    check(texts)?;
    let bytes = texts.iter().fold(0_usize, |a, t| a.saturating_add(t.len()));
    // ALLOC: one block's input, at most 1 MiB of text and 4 bytes of length for each of 64 texts.
    let mut out = Vec::with_capacity(
        bytes
            .saturating_add(texts.len().saturating_mul(4))
            .saturating_add(4),
    );
    out.extend_from_slice(&size(texts.len()).to_le_bytes());
    for t in texts {
        out.extend_from_slice(&size(t.len()).to_le_bytes());
        out.extend_from_slice(t.as_bytes());
    }
    Ok(out)
}

/// The ranges of `texts` that go in one call each: at most
/// [`MAX_BLOCK_TEXTS`] texts and [`MAX_BLOCK_TEXT_BYTES`] bytes, in order and
/// never inside a text.
///
/// # Errors
///
/// A limit failure for a single text over [`MAX_BLOCK_TEXT_BYTES`].
pub fn split_blocks(texts: &[&str]) -> Result<Vec<Range<usize>>, CallError> {
    // ALLOC: one range for each block, at most one for each text.
    let mut out = Vec::new();
    let (mut from, mut bytes) = (0_usize, 0_usize);
    for (i, t) in texts.iter().enumerate() {
        if t.len() > MAX_BLOCK_TEXT_BYTES {
            return Err(limit(b"a text over 1 MiB"));
        }
        let full = i.saturating_sub(from) >= MAX_BLOCK_TEXTS
            || bytes.saturating_add(t.len()) > MAX_BLOCK_TEXT_BYTES;
        if full {
            out.push(from..i);
            from = i;
            bytes = 0;
        }
        bytes = bytes.saturating_add(t.len());
    }
    if from < texts.len() {
        out.push(from..texts.len());
    }
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn word(&mut self) -> Result<u32, CallError> {
        let end = self.at.saturating_add(4);
        let w = self
            .bytes
            .get(self.at..end)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .ok_or_else(|| bad("the output is truncated"))?;
        self.at = end;
        Ok(u32::from_le_bytes(w))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CallError> {
        let end = self.at.saturating_add(n);
        let b = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| bad("the output is truncated"))?;
        self.at = end;
        Ok(b)
    }
}

/// UTF-16 unit offsets of each byte position of `text`, built only for text
/// that is not ASCII.
fn units(text: &str) -> Option<Vec<u32>> {
    if text.is_ascii() {
        return None;
    }
    // ALLOC: one entry for each byte of one text, at most 1 MiB of text.
    let mut at = vec![0_u32; text.len().saturating_add(1)];
    let mut unit = 0_u32;
    for (i, c) in text.char_indices() {
        if let Some(slot) = at.get_mut(i) {
            *slot = unit;
        }
        unit = unit.saturating_add(u32::try_from(c.len_utf16()).unwrap_or(2));
    }
    if let Some(last) = at.last_mut() {
        *last = unit;
    }
    Some(at)
}

fn offset(map: &Option<Vec<u32>>, at: u32) -> u32 {
    match map {
        None => at,
        Some(m) => m.get(at as usize).copied().unwrap_or(at),
    }
}

/// Decodes and validates the output of one call over `texts`.
///
/// # Errors
///
/// A limit failure for more than [`MAX_TOKENS`] tokens; a malformed failure
/// for anything else the contract forbids: an output over
/// [`MAX_OUTPUT_BYTES`], a truncated or trailing output, an empty, non-UTF-8
/// or over-long term, offsets out of order or outside the text or inside a
/// character, a first increment of zero or a position past Lucene's maximum.
pub fn decode_block(texts: &[&str], out: &[u8]) -> Result<Vec<Vec<Token>>, CallError> {
    if out.len() > MAX_OUTPUT_BYTES {
        return Err(bad("the output over 4 MiB"));
    }
    let mut r = Reader { bytes: out, at: 0 };
    // ALLOC: one list for each text of the block, at most 64.
    let mut all = Vec::with_capacity(texts.len());
    let mut total = 0_usize;
    for text in texts {
        let n = r.word()? as usize;
        total = total.saturating_add(n);
        if total > MAX_TOKENS {
            return Err(limit(b"more than 100000 tokens in a call"));
        }
        if r.bytes.len().saturating_sub(r.at) < n.saturating_mul(TOKEN_FIXED) {
            return Err(bad("the output is truncated"));
        }
        let map = units(text);
        // ALLOC: one token each, at most 100,000 a call, and the output is at most 4 MiB.
        let mut toks = Vec::with_capacity(n);
        let (mut position, mut last_start) = (-1_i64, 0_u32);
        for k in 0..n {
            let tlen = r.word()? as usize;
            if tlen == 0 {
                return Err(bad("an empty term"));
            }
            if tlen > MAX_TERM_BYTES {
                return Err(bad("a term over 32766 bytes"));
            }
            let term =
                std::str::from_utf8(r.take(tlen)?).map_err(|_| bad("a term that is not UTF-8"))?;
            let (start, end, inc) = (r.word()?, r.word()?, r.word()?);
            if start > end {
                return Err(bad("a token with start after end"));
            }
            if end as usize > text.len() {
                return Err(bad("a token end past the text"));
            }
            if !text.is_char_boundary(start as usize) || !text.is_char_boundary(end as usize) {
                return Err(bad("an offset inside a character"));
            }
            if start < last_start {
                return Err(bad("a token start that decreases"));
            }
            if k == 0 && inc == 0 {
                return Err(bad("a first increment of zero"));
            }
            position = position.saturating_add(i64::from(inc));
            if position > MAX_POSITION {
                return Err(bad("a position past the maximum"));
            }
            last_start = start;
            toks.push(Token {
                // ALLOC: one term, at most 32,766 bytes, inside the output's 4 MiB.
                term: term.to_owned(),
                start: offset(&map, start),
                end: offset(&map, end),
                increment: inc,
            });
        }
        all.push(toks);
    }
    if r.at != out.len() {
        return Err(bad("trailing bytes after the tokens"));
    }
    Ok(all)
}
