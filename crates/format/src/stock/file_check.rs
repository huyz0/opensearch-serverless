//! The check of a copied stock file: its length and the Lucene footer's
//! CRC, fed the bytes as they stream past, so a file is never held whole.
//!
//! A Lucene file ends in a 16-byte footer: the magic `0xC02893E8`, the
//! algorithm id 0 and the CRC32 of every byte before the last eight. The
//! shard snapshot lists the same CRC in base 36.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "stock file";
/// The bytes of a Lucene footer.
const FOOTER: u64 = 16;
/// The bytes of the footer's checksum.
const CHECKSUM: u64 = 8;
/// The footer's magic.
const MAGIC: [u8; 4] = [0xC0, 0x28, 0x93, 0xE8];

/// A running check of one file's bytes.
#[derive(Debug, Clone)]
pub struct StockFileCheck {
    length: u64,
    listed: u64,
    seen: u64,
    crc: crc32fast::Hasher,
    tail: [u8; 16],
}

impl StockFileCheck {
    /// A check of a file of `length` bytes whose listed checksum is
    /// `checksum`, the CRC in base 36.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a file shorter than a footer or a
    /// checksum that is not a base-36 number of at most 32 bits.
    pub fn new(length: u64, checksum: &str) -> Result<Self, FormatError> {
        if length < FOOTER {
            return Err(corrupt(WHAT, "a file shorter than a Lucene footer"));
        }
        let listed = u64::from_str_radix(checksum, 36)
            .ok()
            .filter(|c| *c <= u64::from(u32::MAX))
            .ok_or_else(|| corrupt(WHAT, "a checksum that is not a 32-bit base-36 number"))?;
        Ok(Self {
            length,
            listed,
            seen: 0,
            crc: crc32fast::Hasher::new(),
            tail: [0; 16],
        })
    }

    /// Starts over, for a retried copy.
    pub fn reset(&mut self) {
        self.seen = 0;
        self.crc = crc32fast::Hasher::new();
        self.tail = [0; 16];
    }

    /// The next bytes of the file.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for more bytes than the file's length.
    pub fn update(&mut self, data: &[u8]) -> Result<(), FormatError> {
        let n = u64::try_from(data.len()).unwrap_or(u64::MAX);
        let end = self.seen.saturating_add(n);
        if end > self.length {
            return Err(corrupt(WHAT, "a file longer than listed"));
        }
        let crc_end = self.length.saturating_sub(CHECKSUM);
        let upto = end.min(crc_end).saturating_sub(self.seen);
        if let Some(part) = data.get(..usize::try_from(upto).unwrap_or(0)) {
            self.crc.update(part);
        }
        let tail_start = self.length.saturating_sub(FOOTER);
        let from = self.seen.max(tail_start);
        // Before the footer `from` is past the data, and the copy is empty.
        let src = usize::try_from(from.saturating_sub(self.seen)).unwrap_or(0);
        let dst = usize::try_from(from.saturating_sub(tail_start)).unwrap_or(0);
        let part = data.get(src..).unwrap_or(&[]);
        if let Some(room) = self.tail.get_mut(dst..dst.saturating_add(part.len())) {
            room.copy_from_slice(part);
        }
        self.seen = end;
        Ok(())
    }

    /// Whether the whole file has been fed and its footer matches.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a file that is short, whose footer has
    /// the wrong magic or algorithm, or whose CRC differs from the footer's
    /// or from the listed one.
    pub fn finish(&self) -> Result<(), FormatError> {
        if self.seen != self.length {
            return Err(corrupt(WHAT, "a file shorter than listed"));
        }
        let (magic, rest) = self.tail.split_at(4);
        let (algorithm, stored) = rest.split_at(4);
        if magic != MAGIC || algorithm != [0, 0, 0, 0] {
            return Err(corrupt(WHAT, "a footer without the Lucene magic"));
        }
        let computed = u64::from(self.crc.clone().finalize());
        let mut raw = [0u8; 8];
        raw.copy_from_slice(stored);
        let stored = u64::from_be_bytes(raw);
        if stored != computed {
            return Err(corrupt(WHAT, "a checksum that does not match the bytes"));
        }
        if self.listed != computed {
            return Err(corrupt(
                WHAT,
                "a checksum that does not match the one listed",
            ));
        }
        Ok(())
    }
}
