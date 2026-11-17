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
use crate::json_write::{push_string, push_u64};

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

/// Puts `files` in the order a blob lays them out: smallest first, equal
/// sizes by name (`SegmentBlobs.prepare`).
pub fn blob_order(files: &mut [(&str, u64)]) {
    files.sort_unstable_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
}

/// The header of a blob holding `files` in this order, with the magic and
/// length before it, as `SegmentBlobs.header` writes it: the files follow
/// it in the same order.
///
/// The offsets count the header itself, whose length depends on how many
/// digits they take, so it is encoded until its length stops moving, which
/// a second or third pass always does.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for no file, a name twice or a range that
/// overflows; [`FormatError::TooLarge`] past [`MAX_FILES`] or
/// [`MAX_HEADER_BYTES`].
pub fn encode_header(files: &[(&str, u64)]) -> Result<Vec<u8>, FormatError> {
    if files.is_empty() {
        return Err(corrupt(WHAT, "a blob names no file"));
    }
    if files.len() > MAX_FILES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_FILES),
            actual: len64(files.len()),
        });
    }
    // ALLOC: one name a file, at most MAX_FILES, for the duplicate check.
    let mut names: Vec<&str> = files.iter().map(|f| f.0).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| matches!(w, [a, b] if a == b)) {
        return Err(corrupt(WHAT, "a file is named twice"));
    }
    // ALLOC: the JSON, under MAX_HEADER_BYTES, rebuilt until its length settles.
    let mut encoded: Vec<u8> = Vec::new();
    for _ in 0..8 {
        let mut offset = len64(PREFIX_BYTES.saturating_add(encoded.len()));
        // ALLOC: the JSON of one pass, under MAX_HEADER_BYTES.
        let mut next: Vec<u8> = Vec::with_capacity(encoded.len().max(64));
        next.extend_from_slice(b"{\"version\":1,\"files\":{");
        for (i, (name, length)) in files.iter().enumerate() {
            if i > 0 {
                next.push(b',');
            }
            push_string(&mut next, name);
            next.push(b':');
            next.push(b'[');
            push_u64(&mut next, offset);
            next.push(b',');
            push_u64(&mut next, *length);
            next.push(b']');
            offset = offset
                .checked_add(*length)
                .ok_or(corrupt(WHAT, "a file range overflows"))?;
        }
        next.extend_from_slice(b"}}");
        if next.len() > usize::try_from(MAX_HEADER_BYTES).unwrap_or(usize::MAX) {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_HEADER_BYTES.into(),
                actual: len64(next.len()),
            });
        }
        if next.len() == encoded.len() {
            // ALLOC: the header, under MAX_HEADER_BYTES, once a blob.
            let mut out = Vec::with_capacity(PREFIX_BYTES.saturating_add(next.len()));
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&u32::try_from(next.len()).unwrap_or(u32::MAX).to_be_bytes());
            out.extend_from_slice(&next);
            return Ok(out);
        }
        encoded = next;
    }
    Err(corrupt(WHAT, "the header's length did not settle"))
}

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
