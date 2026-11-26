//! A SMILE document as JSON text.
//!
//! The binary form of JSON a stock OpenSearch repository writes its
//! metadata in (Jackson's Smile format). [`to_json`] reads one document and
//! writes the JSON it stands for, into the caller's buffer, with every
//! count bounded by [`Limits`] and nothing allocated beyond what the input
//! and the limits allow. Binary values come out as base64 strings, as
//! Jackson's JSON writer writes them.

mod num;
mod write;

pub use write::from_json;

use crate::error::{FormatError, corrupt, len64};

const WHAT: &str = "SMILE document";
const MAX_SHARED: usize = 1024;
const MAX_SHARED_LEN: usize = 64;
const END_STRING: u8 = 0xFC;

/// What a document may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The deepest nesting of arrays and objects.
    pub depth: usize,
    /// The longest string, name or binary, in bytes.
    pub string_bytes: usize,
    /// The most bytes of JSON written (what this call adds to the buffer).
    pub output_bytes: usize,
    /// The most bytes of one big integer.
    pub bigint_bytes: usize,
    /// The most bytes of big integers in the whole document: converting one
    /// to decimal costs time in proportion to its bytes squared.
    pub bigint_total: usize,
    /// The largest decimal scale, in either direction.
    pub scale: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 64,
            string_bytes: 16 << 20,
            output_bytes: 64 << 20,
            bigint_bytes: 32,
            bigint_total: 4096,
            scale: 10_000,
        }
    }
}

/// A position in the input.
pub(crate) struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn byte(&mut self) -> Result<u8, FormatError> {
        let b = *self.buf.get(self.pos).ok_or_else(|| self.short())?;
        self.pos = self.pos.saturating_add(1);
        Ok(b)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.short())?;
        let s = self.buf.get(self.pos..end).ok_or_else(|| self.short())?;
        self.pos = end;
        Ok(s)
    }

    fn short(&self) -> FormatError {
        FormatError::Truncated {
            what: WHAT,
            needed: 0,
            have: len64(self.buf.len()),
        }
    }

    /// The bytes up to the string terminator, past it.
    fn until_end(&mut self, limit: usize) -> Result<&'a [u8], FormatError> {
        let rest = self.buf.get(self.pos..).unwrap_or_default();
        let at = rest
            .iter()
            .take(limit.saturating_add(1))
            .position(|b| *b == END_STRING)
            .ok_or_else(|| corrupt(WHAT, "a long string with no end, or one over its limit"))?;
        let s = rest.get(..at).unwrap_or_default();
        self.pos = self.pos.saturating_add(at).saturating_add(1);
        Ok(s)
    }
}

enum Ctx {
    Array(bool),
    Object(bool),
}

struct Decoder<'a, 'o> {
    c: Cursor<'a>,
    out: &'o mut Vec<u8>,
    base: usize,
    bigint_used: usize,
    lim: Limits,
    shared_names: bool,
    shared_values: bool,
    names: Vec<&'a str>,
    values: Vec<&'a str>,
    stack: Vec<Ctx>,
}

/// Writes the JSON of the SMILE `input` onto `out`.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for bytes that are not a SMILE document (a bad
/// header, a back-reference past its table, invalid UTF-8, a token the
/// format does not define), [`FormatError::Truncated`] for a document that
/// ends early, and [`FormatError::TooLarge`] for one past a limit.
pub fn to_json(input: &[u8], limits: &Limits, out: &mut Vec<u8>) -> Result<(), FormatError> {
    let mut c = Cursor { buf: input, pos: 0 };
    if c.take(3)? != b":)\n" {
        return Err(corrupt(WHAT, "no SMILE header"));
    }
    let flags = c.byte()?;
    if flags >> 4 != 0 {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: u64::from(flags >> 4),
        });
    }
    let mut d = Decoder {
        c,
        base: out.len(),
        bigint_used: 0,
        out,
        lim: *limits,
        shared_names: flags & 1 != 0,
        shared_values: flags & 2 != 0,
        // ALLOC: the shared tables hold at most 1,024 entries and the stack at most lim.depth; all start empty.
        names: Vec::new(),
        values: Vec::new(),
        stack: Vec::new(),
    };
    d.document()
}

impl<'a> Decoder<'a, '_> {
    fn document(&mut self) -> Result<(), FormatError> {
        let first = self.c.byte()?;
        self.value(first)?;
        while let Some(ctx) = self.stack.last_mut() {
            let b = self.c.byte()?;
            match ctx {
                Ctx::Array(first) => {
                    if b == 0xF9 {
                        self.stack.pop();
                        self.put(b"]")?;
                        continue;
                    }
                    if !std::mem::replace(first, false) {
                        self.put(b",")?;
                    }
                    self.value(b)?;
                }
                Ctx::Object(first) => {
                    if b == 0xFB {
                        self.stack.pop();
                        self.put(b"}")?;
                        continue;
                    }
                    if !std::mem::replace(first, false) {
                        self.put(b",")?;
                    }
                    let key = self.key(b)?;
                    self.string(key)?;
                    self.put(b":")?;
                    let v = self.c.byte()?;
                    self.value(v)?;
                }
            }
        }
        // An end marker may follow; nothing else.
        match self.c.buf.get(self.c.pos..) {
            None | Some([]) | Some([0xFF]) => Ok(()),
            Some(_) => Err(corrupt(WHAT, "bytes after the document")),
        }
    }

    /// The bytes this call has added to the buffer.
    fn written(&self) -> usize {
        self.out.len().saturating_sub(self.base)
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        let total = self.written().checked_add(bytes.len());
        if total.is_none_or(|t| t > self.lim.output_bytes) {
            return Err(FormatError::TooLarge {
                what: "JSON from a SMILE document",
                limit: len64(self.lim.output_bytes),
                actual: len64(self.written().saturating_add(bytes.len())),
            });
        }
        // ALLOC: the output, up to lim.output_bytes.
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn open(&mut self, ctx: Ctx, text: &[u8]) -> Result<(), FormatError> {
        if self.stack.len() >= self.lim.depth {
            return Err(FormatError::TooLarge {
                what: "SMILE nesting",
                limit: len64(self.lim.depth),
                actual: len64(self.stack.len().saturating_add(1)),
            });
        }
        self.put(text)?;
        // ALLOC: at most lim.depth entries.
        self.stack.push(ctx);
        Ok(())
    }

    /// One value, its first byte `b` read.
    fn value(&mut self, b: u8) -> Result<(), FormatError> {
        match b {
            0x01..=0x1F => {
                let s = self.shared(
                    &self.values,
                    usize::from(b.saturating_sub(1)),
                    self.shared_values,
                )?;
                self.string(s)
            }
            0x20 => self.put(b"\"\""),
            0x21 => self.put(b"null"),
            0x22 => self.put(b"false"),
            0x23 => self.put(b"true"),
            0x24 | 0x25 => {
                let n = num::unzigzag(num::vint(&mut self.c)?);
                if b == 0x24 && i32::try_from(n).is_err() {
                    return Err(corrupt(WHAT, "a 32-bit integer wider than 32 bits"));
                }
                // ALLOC: an integer's text, at most 20 bytes.
                self.put(n.to_string().as_bytes())
            }
            0x26 => {
                let big = self.big()?;
                self.put(big.as_bytes())
            }
            0x28 => {
                let bits = num::float_bits(&mut self.c, 5)?;
                let bits =
                    u32::try_from(bits).map_err(|_| corrupt(WHAT, "a float wider than 32 bits"))?;
                let f = f32::from_bits(bits);
                // ALLOC: a float's text, at most 25 bytes.
                self.float(f64::from(f), format!("{f:e}"))
            }
            0x29 => {
                let f = f64::from_bits(num::float_bits(&mut self.c, 10)?);
                // ALLOC: a double's text, at most 25 bytes.
                self.float(f, format!("{f:e}"))
            }
            0x2A => {
                let scale = num::unzigzag(num::vint(&mut self.c)?);
                if scale.unsigned_abs() > self.lim.scale.unsigned_abs() {
                    return Err(FormatError::TooLarge {
                        what: "SMILE decimal scale",
                        limit: self.lim.scale.unsigned_abs(),
                        actual: scale.unsigned_abs(),
                    });
                }
                let unscaled = self.big()?;
                self.put(num::with_scale(&unscaled, scale).as_bytes())
            }
            0x40..=0x5F => self.text(usize::from(b & 0x1F).saturating_add(1)),
            0x60..=0x7F => self.text(usize::from(b & 0x1F).saturating_add(33)),
            0x80..=0x9F => self.text(usize::from(b & 0x1F).saturating_add(2)),
            0xA0..=0xBF => self.text(usize::from(b & 0x1F).saturating_add(34)),
            0xC0..=0xDF => {
                let n = num::unzigzag(u64::from(b & 0x1F));
                // ALLOC: an integer's text, at most 20 bytes.
                self.put(n.to_string().as_bytes())
            }
            0xE0 | 0xE4 => {
                let raw = self.c.until_end(self.lim.string_bytes)?;
                let s = std::str::from_utf8(raw)
                    .map_err(|_| corrupt(WHAT, "a string that is not UTF-8"))?;
                self.string(s)
            }
            0xE8 => {
                let n = self.length()?;
                let bytes = num::decode7(&mut self.c, n)?;
                self.base64(&bytes)
            }
            0xEC..=0xEF => {
                let index = usize::from(b & 3)
                    .wrapping_shl(8)
                    .wrapping_add(usize::from(self.c.byte()?));
                let s = self.shared(&self.values, index, self.shared_values)?;
                self.string(s)
            }
            0xF8 => self.open(Ctx::Array(true), b"["),
            0xFA => self.open(Ctx::Object(true), b"{"),
            0xFD => {
                let n = self.length()?;
                let raw = self.c.take(n)?;
                self.base64(raw)
            }
            _ => Err(corrupt(WHAT, "a token the format does not define")),
        }
    }

    /// An object key, its first byte `b` read.
    fn key(&mut self, b: u8) -> Result<&'a str, FormatError> {
        match b {
            0x20 => Ok(""),
            0x30..=0x33 => {
                let index = usize::from(b & 3)
                    .wrapping_shl(8)
                    .wrapping_add(usize::from(self.c.byte()?));
                self.shared(&self.names, index, self.shared_names)
            }
            0x34 => {
                let raw = self.c.until_end(self.lim.string_bytes)?;
                let s = std::str::from_utf8(raw)
                    .map_err(|_| corrupt(WHAT, "a name that is not UTF-8"))?;
                // A long name joins the table too, as Jackson's reader adds it.
                if self.shared_names {
                    remember(&mut self.names, s);
                }
                Ok(s)
            }
            0x40..=0x7F => self.shared(&self.names, usize::from(b & 0x3F), self.shared_names),
            0x80..=0xBF => self.name(usize::from(b & 0x3F).saturating_add(1)),
            0xC0..=0xF7 => self.name(usize::from(b.saturating_sub(0xC0)).saturating_add(2)),
            _ => Err(corrupt(WHAT, "a token that is not an object name")),
        }
    }

    /// A name of `len` bytes. It joins the table either way: a document
    /// that does not share names never reads the table.
    fn name(&mut self, len: usize) -> Result<&'a str, FormatError> {
        let raw = self.c.take(len)?;
        let s = std::str::from_utf8(raw).map_err(|_| corrupt(WHAT, "a name that is not UTF-8"))?;
        if len <= MAX_SHARED_LEN {
            remember(&mut self.names, s);
        }
        Ok(s)
    }

    fn text(&mut self, len: usize) -> Result<(), FormatError> {
        let raw = self.c.take(len)?;
        let s =
            std::str::from_utf8(raw).map_err(|_| corrupt(WHAT, "a string that is not UTF-8"))?;
        if len <= MAX_SHARED_LEN {
            remember(&mut self.values, s);
        }
        self.string(s)
    }

    fn shared(&self, table: &[&'a str], index: usize, on: bool) -> Result<&'a str, FormatError> {
        if !on {
            return Err(corrupt(
                WHAT,
                "a back-reference in a document without sharing",
            ));
        }
        table
            .get(index)
            .copied()
            .ok_or_else(|| corrupt(WHAT, "a back-reference past its table"))
    }

    fn length(&mut self) -> Result<usize, FormatError> {
        let n = num::vint(&mut self.c)?;
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        if n > self.lim.string_bytes {
            return Err(FormatError::TooLarge {
                what: "SMILE binary",
                limit: len64(self.lim.string_bytes),
                actual: len64(n),
            });
        }
        Ok(n)
    }

    /// A big integer as decimal text.
    fn big(&mut self) -> Result<String, FormatError> {
        let n = num::vint(&mut self.c)?;
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        if n > self.lim.bigint_bytes {
            return Err(FormatError::TooLarge {
                what: "SMILE big integer",
                limit: len64(self.lim.bigint_bytes),
                actual: len64(n),
            });
        }
        self.bigint_used = self.bigint_used.saturating_add(n);
        if self.bigint_used > self.lim.bigint_total {
            return Err(FormatError::TooLarge {
                what: "SMILE big integers in one document",
                limit: len64(self.lim.bigint_total),
                actual: len64(self.bigint_used),
            });
        }
        Ok(num::decimal(&num::decode7(&mut self.c, n)?))
    }

    fn float(&mut self, f: f64, text: String) -> Result<(), FormatError> {
        if !f.is_finite() {
            return Err(corrupt(WHAT, "a number that is not finite"));
        }
        self.put(text.as_bytes())
    }

    /// Binary as a base64 string, written a group at a time so the output
    /// cap is met before more is encoded than fits.
    fn base64(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        self.put(b"\"")?;
        for chunk in bytes.chunks(3) {
            self.put(&num::base64_group(chunk))?;
        }
        self.put(b"\"")
    }

    /// A JSON string, escaped.
    fn string(&mut self, s: &str) -> Result<(), FormatError> {
        self.put(b"\"")?;
        let mut start = 0usize;
        for (i, ch) in s.char_indices() {
            let esc: Option<&[u8]> = match ch {
                '"' => Some(b"\\\""),
                '\\' => Some(b"\\\\"),
                '\n' => Some(b"\\n"),
                '\r' => Some(b"\\r"),
                '\t' => Some(b"\\t"),
                c if (c as u32) < 0x20 => None,
                _ => continue,
            };
            self.put(s.get(start..i).unwrap_or_default().as_bytes())?;
            match esc {
                Some(e) => self.put(e)?,
                // ALLOC: six bytes of escape.
                None => self.put(format!("\\u{:04x}", ch as u32).as_bytes())?,
            }
            start = i.saturating_add(ch.len_utf8());
        }
        self.put(s.get(start..).unwrap_or_default().as_bytes())?;
        self.put(b"\"")
    }
}

/// Adds to a shared table, which restarts when it is full.
fn remember<'a>(table: &mut Vec<&'a str>, s: &'a str) {
    if table.len() >= MAX_SHARED {
        table.clear();
    }
    // ALLOC: at most MAX_SHARED entries.
    table.push(s);
}
