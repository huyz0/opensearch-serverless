//! A shard's published commit: its term, and where each segment file is.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/store/CommitManifest.java`
//! (`fromStream`). The manifest is the value of the shard's commit register,
//! JSON with these fields, any order:
//!
//! - `term` (required): the publishing writer's term;
//! - `writer`: its node id, or null;
//! - `files` (required): file name to [`FileLocation`] text;
//! - `lengths`: file name to length, for whichever files are known;
//! - `wal_ordinal`, `stage_seq`: how far into the write log the commit
//!   reaches, and its place among staged commits; negative when absent;
//! - `digest`: the pruning digest, which this reader skips;
//! - anything else is skipped, as the Java skips it.
//!
//! Differences from the Java, each stricter: a file named twice in `files` or
//! `lengths` is refused (the Java keeps the last), a recorded length that
//! disagrees with the location's range is refused (the Java prefers the
//! recorded one), and more than [`MAX_FILES`] files is refused.

use std::borrow::Cow;

use crate::error::{FormatError, corrupt, len64};
use crate::json::{Reader, Token};
use crate::location::FileLocation;

const WHAT: &str = "commit manifest";

/// The most files one manifest may name. A Lucene commit holds about a dozen
/// files a segment, and a shard keeps tens of segments.
pub const MAX_FILES: usize = 16_384;

/// One file a manifest names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile<'a> {
    /// The Lucene file name.
    pub name: Cow<'a, str>,
    /// Where its bytes are, as recorded.
    pub location: FileLocation<'a>,
    /// Its length: the recorded one, else the location's range length.
    pub length: Option<u64>,
}

/// A parsed commit manifest, borrowing from the register's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitManifest<'a> {
    /// The publishing writer's term.
    pub term: u64,
    /// The publishing node, when recorded.
    pub writer: Option<Cow<'a, str>>,
    /// The highest write-log ordinal of `term` the commit holds, or negative.
    pub wal_ordinal: i64,
    /// The commit's place among those its writer staged, or negative.
    pub stage_sequence: i64,
    /// Whether the manifest carries a pruning digest.
    pub has_digest: bool,
    files: Vec<ManifestFile<'a>>,
}

impl<'a> CommitManifest<'a> {
    /// Parses a manifest from the register's value (after the generation).
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for malformed JSON, a missing `term` or
    /// `files`, a bad location or length, or a file named twice;
    /// [`FormatError::TooLarge`] past [`MAX_FILES`].
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        let mut json = Reader::new(value, WHAT);
        json.start_object("expected an object")?;
        let mut term: Option<i64> = None;
        let mut writer = None;
        let mut wal_ordinal = -1i64;
        let mut stage_sequence = -1i64;
        let mut has_digest = false;
        let mut saw_files = false;
        // ALLOC: one entry a file, at most MAX_FILES (checked before each push).
        let mut files: Vec<ManifestFile<'a>> = Vec::new();
        // ALLOC: one entry a recorded length, at most MAX_FILES (checked before each push).
        let mut lengths: Vec<(Cow<'a, str>, u64)> = Vec::new();
        while let Some(field) = json.next_field()? {
            if !matches!(
                field.as_ref(),
                "files" | "lengths" | "digest" | "term" | "wal_ordinal" | "stage_seq" | "writer"
            ) {
                json.skip_value()?;
                continue;
            }
            if field == "digest" {
                if !json.next_is_object() {
                    return Err(json.error("digest must be an object"));
                }
                has_digest = true;
                json.skip_value()?;
                continue;
            }
            let token = json.expect_token()?;
            match field.as_ref() {
                "files" => {
                    expect_object(&json, &token, "files must be an object")?;
                    saw_files = true;
                    read_files(&mut json, &mut files)?;
                }
                "lengths" => {
                    expect_object(&json, &token, "lengths must be an object")?;
                    read_lengths(&mut json, &mut lengths)?;
                }
                "term" | "wal_ordinal" | "stage_seq" => {
                    let Token::Int(number) = token else {
                        return Err(json.error("term, wal_ordinal and stage_seq must be integers"));
                    };
                    match field.as_ref() {
                        "term" => term = Some(number),
                        "wal_ordinal" => wal_ordinal = number,
                        _ => stage_sequence = number,
                    }
                }
                "writer" if token == Token::Null => writer = None,
                _ => {
                    let Token::Str(name) = token else {
                        return Err(json.error("writer must be a string"));
                    };
                    writer = Some(name);
                }
            }
        }
        json.finish()?;
        let term = match term {
            Some(t) if t >= 0 && saw_files => u64::try_from(t).unwrap_or_default(),
            _ => return Err(corrupt(WHAT, "missing term or files")),
        };
        files.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        refuse_duplicates(
            files
                .windows(2)
                .any(|w| matches!(w, [a, b] if a.name == b.name)),
        )?;
        lengths.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        refuse_duplicates(
            lengths
                .windows(2)
                .any(|w| matches!(w, [a, b] if a.0 == b.0)),
        )?;
        for (name, length) in lengths {
            // A length for a file `files` does not name is kept by the Java and never read.
            let found = files.binary_search_by(|f| f.name.as_ref().cmp(name.as_ref()));
            let Some(file) = found.ok().and_then(|at| files.get_mut(at)) else {
                continue;
            };
            if file.length.is_some_and(|ranged| ranged != length) {
                return Err(corrupt(
                    WHAT,
                    "a recorded length disagrees with the file's range",
                ));
            }
            file.length = Some(length);
        }
        Ok(Self {
            term,
            writer,
            wal_ordinal,
            stage_sequence,
            has_digest,
            files,
        })
    }

    /// Every file, in name order.
    pub fn files(&self) -> &[ManifestFile<'a>] {
        &self.files
    }

    /// One file, if the manifest names it.
    pub fn file(&self, name: &str) -> Option<&ManifestFile<'a>> {
        self.files
            .binary_search_by(|f| f.name.as_ref().cmp(name))
            .ok()
            .and_then(|at| self.files.get(at))
    }
}

fn refuse_duplicates(found: bool) -> Result<(), FormatError> {
    if found {
        Err(corrupt(WHAT, "a file is named twice"))
    } else {
        Ok(())
    }
}

fn expect_object(
    json: &Reader<'_>,
    token: &Token<'_>,
    reason: &'static str,
) -> Result<(), FormatError> {
    if *token == Token::StartObject {
        Ok(())
    } else {
        Err(json.error(reason))
    }
}

fn check_count(count: usize) -> Result<(), FormatError> {
    if count >= MAX_FILES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: len64(MAX_FILES),
            actual: len64(count.saturating_add(1)),
        });
    }
    Ok(())
}

fn read_files<'a>(
    json: &mut Reader<'a>,
    files: &mut Vec<ManifestFile<'a>>,
) -> Result<(), FormatError> {
    loop {
        let Some(name) = json.next_field()? else {
            return Ok(());
        };
        let Token::Str(Cow::Borrowed(recorded)) = json.expect_token()? else {
            // An escaped location would need an owned copy for the location to
            // borrow from; the shell never writes one, since locations are ASCII.
            return Err(json.error("expected a file location"));
        };
        let location = FileLocation::parse(recorded)?;
        check_count(files.len())?;
        files.push(ManifestFile {
            name,
            length: location.range().map(|(_, length)| length),
            location,
        });
    }
}

fn read_lengths<'a>(
    json: &mut Reader<'a>,
    lengths: &mut Vec<(Cow<'a, str>, u64)>,
) -> Result<(), FormatError> {
    loop {
        let Some(name) = json.next_field()? else {
            return Ok(());
        };
        let Token::Int(length) = json.expect_token()? else {
            return Err(json.error("expected a file length"));
        };
        let length = u64::try_from(length).map_err(|_| json.error("negative file length"))?;
        check_count(lengths.len())?;
        lengths.push((name, length));
    }
}
