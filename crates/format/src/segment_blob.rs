//! A segment blob: one immutable blob holding one segment's files.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/store/SegmentBlobs.java`
//! (`header`). The blob starts with `SEG1`, then the length of a JSON header
//! as a big-endian 32-bit integer, then the header:
//!
//! ```json
//! {"version":1,"files":{"_0.si":[8+len, 345], "_0.cfe":[..., ...]}}
//! ```
//!
//! naming each file's offset from the blob's start and its length, smallest
//! file first. The files follow the header in that order. A commit manifest
//! already records each file's range, so a reader needs this header only to
//! check a blob, or to open one without its manifest.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt, len64};
use crate::json::{Reader, Token};

const WHAT: &str = "segment blob header";

/// The magic a segment blob starts with.
pub const MAGIC: [u8; 4] = *b"SEG1";

/// Bytes before the JSON: the magic and its length.
pub const PREFIX_BYTES: usize = 8;

/// The longest JSON header accepted. A segment has about a dozen files, whose
/// entries take under 100 bytes each.
pub const MAX_HEADER_BYTES: u32 = 64 * 1024;

/// The most files one segment blob may name.
pub const MAX_FILES: usize = 1_024;

/// One file in a segment blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFile<'a> {
    /// The Lucene file name.
    pub name: Cow<'a, str>,
    /// Where it starts, from the blob's first byte.
    pub offset: u64,
    /// Its length.
    pub length: u64,
}

/// A parsed segment blob header, files in blob order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentBlobHeader<'a> {
    files: Vec<SegmentFile<'a>>,
    header_end: u64,
}

/// How many bytes from the blob's start the header takes, from its first
/// [`PREFIX_BYTES`].
///
/// # Errors
///
/// [`FormatError::Truncated`] for fewer than [`PREFIX_BYTES`] bytes,
/// [`FormatError::Corrupt`] for a wrong magic, [`FormatError::TooLarge`] past
/// [`MAX_HEADER_BYTES`].
pub fn header_end(leading: &[u8]) -> Result<usize, FormatError> {
    let (Some(magic), Some(length)) = (leading.get(..4), leading.get(4..PREFIX_BYTES)) else {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: len64(PREFIX_BYTES),
            have: len64(leading.len()),
        });
    };
    if magic != MAGIC {
        return Err(corrupt(WHAT, "not a segment blob: wrong magic"));
    }
    let length = u32::from_be_bytes(length.try_into().unwrap_or_default());
    // Java writes a signed int; anything past MAX_HEADER_BYTES, negative included, is refused.
    if length > MAX_HEADER_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_HEADER_BYTES.into(),
            actual: length.into(),
        });
    }
    usize::try_from(length)
        .ok()
        .and_then(|l| l.checked_add(PREFIX_BYTES))
        .ok_or(corrupt(WHAT, "header length"))
}

impl<'a> SegmentBlobHeader<'a> {
    /// Parses the header from the blob's leading bytes, which must hold at
    /// least [`header_end`] of them; more are ignored.
    ///
    /// # Errors
    ///
    /// [`FormatError::Truncated`] with `needed` set when `leading` is shorter
    /// than the header; [`FormatError::Unsupported`] for a version other than
    /// 1; [`FormatError::Corrupt`] for malformed JSON, a file range that
    /// starts inside the header, overlaps the one before, or overflows.
    pub fn parse(leading: &'a [u8]) -> Result<Self, FormatError> {
        let end = header_end(leading)?;
        let Some(body) = leading.get(PREFIX_BYTES..end) else {
            return Err(FormatError::Truncated {
                what: WHAT,
                needed: len64(end),
                have: len64(leading.len()),
            });
        };
        let mut json = Reader::new(body, WHAT);
        json.start_object("expected an object")?;
        let mut version = None;
        // ALLOC: one entry a file, at most MAX_FILES (checked before each push).
        let mut files: Vec<SegmentFile<'a>> = Vec::new();
        let mut saw_files = false;
        while let Some(field) = json.next_field()? {
            if field != "version" && field != "files" {
                json.skip_value()?;
                continue;
            }
            let token = json.expect_token()?;
            match (field.as_ref(), token) {
                ("version", Token::Int(v)) => version = Some(v),
                ("version", _) => return Err(json.error("version must be an integer")),
                ("files", Token::StartObject) => {
                    saw_files = true;
                    read_files(&mut json, &mut files)?;
                }
                _ => return Err(json.error("files must be an object")),
            }
        }
        json.finish()?;
        match version {
            Some(1) => {}
            Some(other) => {
                return Err(FormatError::Unsupported {
                    what: WHAT,
                    version: other.unsigned_abs(),
                });
            }
            None => return Err(corrupt(WHAT, "missing version")),
        }
        if !saw_files {
            return Err(corrupt(WHAT, "missing files"));
        }
        let header_end = len64(end);
        let mut next_free = header_end;
        for file in &files {
            if file.offset < next_free {
                return Err(corrupt(
                    WHAT,
                    "a file starts inside the header or the file before it",
                ));
            }
            next_free = file
                .offset
                .checked_add(file.length)
                .ok_or(corrupt(WHAT, "a file range overflows"))?;
        }
        Ok(Self { files, header_end })
    }

    /// The files, in blob order.
    pub fn files(&self) -> &[SegmentFile<'a>] {
        &self.files
    }

    /// One file by name.
    pub fn file(&self, name: &str) -> Option<&SegmentFile<'a>> {
        self.files.iter().find(|f| f.name == name)
    }

    /// Where the header ends and the first file may start.
    pub fn header_end(&self) -> u64 {
        self.header_end
    }

    /// The blob's length implied by the header: the end of its last file.
    pub fn blob_length(&self) -> u64 {
        self.files
            .last()
            .map_or(self.header_end, |f| f.offset.saturating_add(f.length))
    }
}

fn read_files<'a>(
    json: &mut Reader<'a>,
    files: &mut Vec<SegmentFile<'a>>,
) -> Result<(), FormatError> {
    loop {
        let Some(name) = json.next_field()? else {
            return Ok(());
        };
        if json.expect_token()? != Token::StartArray {
            return Err(json.error("a file is [offset, length]"));
        }
        let first = json.expect_token()?;
        let offset = non_negative(json, first)?;
        let second = json.expect_token()?;
        let length = non_negative(json, second)?;
        if json.expect_token()? != Token::EndArray {
            return Err(json.error("a file is [offset, length]"));
        }
        if files.iter().any(|f| f.name == name) {
            return Err(json.error("a file is named twice"));
        }
        if files.len() >= MAX_FILES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: len64(MAX_FILES),
                actual: len64(files.len().saturating_add(1)),
            });
        }
        files.push(SegmentFile {
            name,
            offset,
            length,
        });
    }
}

fn non_negative(json: &Reader<'_>, token: Token<'_>) -> Result<u64, FormatError> {
    match token {
        Token::Int(n) => {
            u64::try_from(n).map_err(|_| json.error("a file's offset or length is negative"))
        }
        _ => Err(json.error("a file is [offset, length]")),
    }
}
