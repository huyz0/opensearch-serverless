//! A shard's snapshot blob (`snap-<uuid>.dat`): the files one snapshot holds
//! of one shard and where their bytes are stored.
//!
//! Ported from `BlobStoreIndexShardSnapshot.fromXContent` and `FileInfo`.
//! The input is the JSON of the decoded blob.

use crate::error::{FormatError, corrupt};
use crate::json::{Reader, Token};

use super::repository_data::is_blob_name;

const WHAT: &str = "stock shard snapshot";

/// The most files one shard snapshot lists.
pub const MAX_FILES: usize = 10_000;
/// The most chunks one file is stored in.
pub const MAX_CHUNKS: u64 = 10_000;
/// The most bytes of a file's inline copy (`meta_hash`).
const MAX_HASH_BYTES: usize = 64 << 10;
/// What a virtual file's blob name starts with: such a file has no blob, its
/// bytes are the inline copy.
const VIRTUAL_PREFIX: &str = "v__";

/// One file of a shard snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockFile {
    /// The blob the bytes are stored under (without a `.partN` suffix).
    pub name: String,
    /// The Lucene file name it is restored as.
    pub physical_name: String,
    /// Its length in bytes.
    pub length: u64,
    /// Its Lucene checksum, as the footer's CRC in base 36.
    pub checksum: String,
    /// The size of each stored chunk, when it is split.
    pub part_size: Option<u64>,
    /// The Lucene version that wrote it.
    pub written_by: Option<String>,
    /// The inline copy of the file, for a small one.
    pub inline: Option<Vec<u8>>,
}

impl StockFile {
    /// Whether the file has no blob: its bytes are `inline`.
    pub fn is_virtual(&self) -> bool {
        self.name.starts_with(VIRTUAL_PREFIX)
    }

    /// How many blobs hold the file: one, or `ceil(length / part_size)`.
    pub fn parts(&self) -> u64 {
        match self.part_size {
            Some(size) if self.length > 0 => {
                // `size` is at least 1.
                self.length
                    .saturating_sub(1)
                    .checked_div(size)
                    .map_or(1, |d| d.saturating_add(1))
            }
            _ => 1,
        }
    }

    /// The blob name of chunk `part`: the name alone for an unsplit file.
    pub fn part_name(&self, part: u64) -> String {
        if self.parts() > 1 {
            // ALLOC: one blob name, at most 140 bytes.
            format!("{}.part{part}", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// A parsed shard snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StockShardSnapshot {
    /// The snapshot's name.
    pub name: String,
    /// The shard's files.
    pub files: Vec<StockFile>,
}

impl StockShardSnapshot {
    /// Reads the JSON of a shard snapshot blob.
    pub fn parse(json: &[u8]) -> Result<Self, FormatError> {
        let mut r = Reader::new(json, WHAT);
        r.start_object("a shard snapshot is an object")?;
        let mut out = Self::default();
        while let Some(field) = r.next_field()? {
            match field.as_ref() {
                "name" => match r.expect_token()? {
                    Token::Str(s) => out.name = s.into_owned(),
                    _ => return Err(r.error("a name is a string")),
                },
                "files" => out.files = files(&mut r)?,
                _ => r.skip_value()?,
            }
        }
        r.finish()?;
        Ok(out)
    }
}

fn files(r: &mut Reader<'_>) -> Result<Vec<StockFile>, FormatError> {
    match r.expect_token()? {
        Token::StartArray => {}
        _ => return Err(r.error("files is an array")),
    }
    // ALLOC: one entry a file, at most MAX_FILES.
    let mut out = Vec::new();
    loop {
        match r.expect_token()? {
            Token::EndArray => return Ok(out),
            Token::StartObject => {}
            _ => return Err(r.error("a file is an object")),
        }
        if out.len() >= MAX_FILES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_FILES as u64,
                actual: (MAX_FILES as u64).saturating_add(1),
            });
        }
        out.push(file(r)?);
    }
}

fn file(r: &mut Reader<'_>) -> Result<StockFile, FormatError> {
    let (mut name, mut physical, mut length, mut checksum) = (None, None, None, None);
    let (mut part_size, mut written_by, mut inline) = (None, None, None);
    while let Some(field) = r.next_field()? {
        match field.as_ref() {
            "name" => name = Some(text(r, true)?),
            "physical_name" => physical = Some(text(r, true)?),
            "checksum" => checksum = Some(text(r, false)?),
            "written_by" => written_by = Some(text(r, false)?),
            "length" => length = Some(number(r)?),
            "part_size" => part_size = Some(number(r)?).filter(|n| *n > 0),
            "meta_hash" => inline = Some(base64(&text(r, false)?, r)?),
            _ => r.skip_value()?,
        }
    }
    let (Some(name), Some(physical_name), Some(length), Some(checksum)) =
        (name, physical, length, checksum)
    else {
        return Err(r.error("a file without a name, length or checksum"));
    };
    let file = StockFile {
        name,
        physical_name,
        length,
        checksum,
        part_size,
        written_by,
        inline,
    };
    if file.parts() > MAX_CHUNKS {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_CHUNKS,
            actual: file.parts(),
        });
    }
    if file.is_virtual() && file.inline.as_ref().map(Vec::len) != usize::try_from(file.length).ok()
    {
        return Err(corrupt(
            WHAT,
            "a virtual file whose inline copy is not its length",
        ));
    }
    Ok(file)
}

/// A string; `key` for one that becomes part of a blob key.
fn text(r: &mut Reader<'_>, key: bool) -> Result<String, FormatError> {
    match r.expect_token()? {
        Token::Str(s) if !key || is_blob_name(&s) => Ok(s.into_owned()),
        Token::Str(_) => Err(r.error("a name that is not safe in a blob key")),
        _ => Err(r.error("expected a string")),
    }
}

fn number(r: &mut Reader<'_>) -> Result<u64, FormatError> {
    match r.expect_token()? {
        Token::Int(n) => u64::try_from(n).map_err(|_| r.error("a negative length")),
        _ => Err(r.error("expected an integer")),
    }
}

/// Standard base64, as Jackson writes a byte array.
fn base64(s: &str, r: &Reader<'_>) -> Result<Vec<u8>, FormatError> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bad = || r.error("an inline copy that is not base64");
    let s = s.trim_end_matches('=').as_bytes();
    let decoded = s.len().saturating_mul(3).checked_div(4).unwrap_or(0);
    if decoded > MAX_HASH_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_HASH_BYTES as u64,
            actual: decoded as u64,
        });
    }
    // ALLOC: at most MAX_HASH_BYTES, checked above.
    let mut out = Vec::with_capacity(decoded);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for b in chunk {
            let v = A.iter().position(|c| c == b).ok_or_else(bad)?;
            n = n
                .wrapping_shl(6)
                .wrapping_add(u32::try_from(v).unwrap_or(0));
        }
        // The group left-aligned in 24 bits, and how many bytes it holds.
        let (aligned, bytes, unused) = match chunk.len() {
            4 => (n, 3usize, 0u32),
            3 => (n.wrapping_shl(6), 2, 0xFF),
            2 => (n.wrapping_shl(12), 1, 0xFFFF),
            _ => return Err(bad()),
        };
        if aligned & unused != 0 {
            return Err(bad());
        }
        for shift in [16u32, 8, 0].into_iter().take(bytes) {
            out.push(u8::try_from(aligned.wrapping_shr(shift) & 0xFF).unwrap_or(0));
        }
    }
    Ok(out)
}
