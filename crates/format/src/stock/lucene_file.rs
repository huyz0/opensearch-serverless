//! The Lucene version a file records, read from the head of a segment's
//! `.si` or of a `segments_N`.
//!
//! Both start with a Lucene index header: the magic, the codec name, a
//! version, a 16-byte id and a suffix. A `.si` then writes the version that
//! wrote the segment as three big-endian ints (major, minor, bugfix);
//! `segments_N` writes it as three variable-length ints. Stock records it
//! as each file's `written_by`.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "Lucene file head";
const MAGIC: [u8; 4] = [0x3F, 0xD7, 0x6C, 0x17];
/// The id every index header carries.
const ID_BYTES: usize = 16;
/// The longest codec name or suffix read.
const MAX_NAME: usize = 127;
/// The newest major version accepted: Lucene is at 10.
const MAX_MAJOR: u32 = 99;

/// A cursor over the head.
struct Head<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Head<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.at.saturating_add(n);
        let part = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| corrupt(WHAT, "a head cut short"))?;
        self.at = end;
        Ok(part)
    }

    fn byte(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn int(&mut self) -> Result<u32, FormatError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
            b.get(2).copied().unwrap_or(0),
            b.get(3).copied().unwrap_or(0),
        ]))
    }

    /// A little-endian int, as a segment info's version fields are written.
    fn int_le(&mut self) -> Result<u32, FormatError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
            b.get(2).copied().unwrap_or(0),
            b.get(3).copied().unwrap_or(0),
        ]))
    }

    /// A variable-length int of at most five bytes.
    fn vint(&mut self) -> Result<u32, FormatError> {
        let mut value = 0u32;
        for shift in [0u32, 7, 14, 21, 28] {
            let b = self.byte()?;
            value |= u32::from(b & 0x7F).wrapping_shl(shift);
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(corrupt(WHAT, "a variable-length int that does not end"))
    }

    /// An index header: the magic, the codec, the version, the id and the
    /// suffix.
    fn header(&mut self) -> Result<(), FormatError> {
        if self.take(4)? != MAGIC {
            return Err(corrupt(WHAT, "no Lucene header magic"));
        }
        let codec = usize::from(self.byte()?);
        if codec == 0 || codec > MAX_NAME {
            return Err(corrupt(WHAT, "a codec name of an impossible length"));
        }
        self.take(codec)?;
        self.int()?;
        self.take(ID_BYTES)?;
        let suffix = usize::from(self.byte()?);
        if suffix > MAX_NAME {
            return Err(corrupt(WHAT, "a suffix of an impossible length"));
        }
        self.take(suffix)?;
        Ok(())
    }
}

/// The version `major.minor.bugfix` a file's head records: of a segment's
/// `.si` when `segment_info`, of a `segments_N` otherwise.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a head that is cut short, is not a Lucene
/// header, or records a major version of 0 or past 99.
pub fn lucene_version_of(head: &[u8], segment_info: bool) -> Result<String, FormatError> {
    let mut h = Head { bytes: head, at: 0 };
    h.header()?;
    let (major, minor, bugfix) = if segment_info {
        (h.int_le()?, h.int_le()?, h.int_le()?)
    } else {
        (h.vint()?, h.vint()?, h.vint()?)
    };
    if major == 0 || major > MAX_MAJOR || minor > MAX_MAJOR.saturating_mul(100) || bugfix > 9_999 {
        return Err(corrupt(WHAT, "a version that is not a Lucene one"));
    }
    Ok(format!("{major}.{minor}.{bugfix}")) // ALLOC: one short version string.
}
