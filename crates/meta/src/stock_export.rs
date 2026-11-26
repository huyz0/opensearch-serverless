//! Copying one shard's published commit into a stock repository: each file
//! is read in ranges of one buffer and created as one `__<id>` blob under
//! the shard's directory, so memory is two buffers whatever a file's length.
//!
//! The Lucene footer CRC and the length are measured as the bytes pass; the
//! CRC becomes the checksum the shard snapshot lists, and the Lucene
//! version of each segment is read from the head of its `.si` (or of
//! `segments_N`) kept on the way. No LIST: the commit names every file.

use std::collections::HashMap;

use oss_blob::{BlobError, BlobResult, BlobStore, ChunkSource, CreateOutcome};
use oss_format::{
    FileLocation, FormatError, ShardCapture, ShardRef, StockFileCheck, is_blob_name,
    lucene_version_of, stock_checksum_string,
};

use crate::pit::new_id;
use crate::ranged_source::RangedSource;
use crate::snapshot_restore::CopyBuffers;
use crate::snapshot_take::SnapshotTaker;

/// How much of a file's start is kept to read its Lucene version from: a
/// header with the longest codec name and suffix, and three version fields.
const HEAD_BYTES: usize = 320;

/// The most files one shard's commit may list, as the shard snapshot writer
/// bounds them.
const MAX_FILES: usize = 10_000;

/// The repository an export writes into: a store the node writes under
/// `prefix` only (an [`oss_blob::ExportStore`]), and that prefix (empty, or
/// ending in `/`).
pub struct ExportTarget<'a> {
    /// The target store.
    pub store: &'a dyn BlobStore,
    /// The repository's prefix in it.
    pub prefix: &'a str,
}

/// A file copied to the target, as the shard snapshot lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    /// The blob it was created as (`__<id>`).
    pub name: String,
    /// The Lucene file name it is restored as.
    pub physical_name: String,
    /// Its length in bytes.
    pub length: u64,
    /// Its footer CRC in base 36.
    pub checksum: String,
    /// The Lucene version that wrote it.
    pub written_by: String,
}

/// Why a shard could not be exported.
#[derive(Debug)]
pub enum ExportError {
    /// The commit holds something an export does not carry.
    Unsupported(&'static str),
    /// A file's bytes did not match what the commit recorded.
    Mismatch(FormatError),
    /// A record or key was malformed.
    Format(FormatError),
    /// The store failed.
    Blob(BlobError),
    /// The target repository could not be read.
    Read(crate::stock_read::StockReadError),
    /// A snapshot of that name or uuid is already in the repository.
    NameTaken,
    /// Other takes into the repository kept winning, or took the index's id
    /// from this one; nothing names this take's files.
    Conflict,
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) => write!(f, "cannot export: {why}"),
            Self::Mismatch(e) => write!(f, "a copied file does not match its record: {e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::Blob(e) => write!(f, "{e}"),
            Self::Read(e) => write!(f, "{e}"),
            Self::NameTaken => write!(f, "a snapshot of that name is already in the repository"),
            Self::Conflict => write!(f, "the repository changed under the take"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<BlobError> for ExportError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for ExportError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

/// A file's bytes as a body, measured as they pass.
struct MeasuredSource<'a> {
    inner: RangedSource<'a>,
    check: StockFileCheck,
}

impl ChunkSource for MeasuredSource<'_> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let n = self.inner.fill(buf)?;
        self.check
            .update(buf.get(..n).unwrap_or(&[]))
            .map_err(|_| BlobError::Stream("a captured file is longer than recorded"))?;
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.inner.rewind()?;
        self.check.reset();
        Ok(())
    }
}

/// The segment a file belongs to (`_0` for `_0.cfs` and `_0_1.liv`), none for
/// a name that is no segment's.
fn segment_of(physical: &str) -> Option<&str> {
    let rest = physical.strip_prefix('_')?;
    let end = rest.find(['_', '.'])?;
    physical.get(..end.saturating_add(1))
}

impl SnapshotTaker {
    /// Copies the files of `capture`, the commit of the shard `from` names, into the target
    /// repository's `indices/<index_id>/<shard>/`. Cost: one ranged GET per
    /// buffer of each file and one conditional create per file.
    ///
    /// # Errors
    ///
    /// [`ExportError::Mismatch`] for a file whose length or footer CRC is
    /// not what the commit recorded; [`ExportError::Unsupported`] for a file
    /// no Lucene version can be found for; the store's and the record's own.
    pub fn export_shard(
        &self,
        from: ShardRef<'_>,
        capture: &ShardCapture,
        target: &ExportTarget<'_>,
        index_id: &str,
        buffers: &mut CopyBuffers<'_>,
    ) -> Result<Vec<ExportedFile>, ExportError> {
        if from.shard != capture.shard {
            return Err(ExportError::Unsupported("a capture of another shard"));
        }
        if index_id.is_empty() || !is_blob_name(index_id) {
            return Err(ExportError::Unsupported("an index id that is no safe name"));
        }
        if !target.prefix.is_empty() && !target.prefix.ends_with('/') {
            return Err(ExportError::Unsupported("a prefix without a closing slash"));
        }
        if capture.files.len() > MAX_FILES {
            return Err(ExportError::Unsupported("a commit of too many files"));
        }
        // ALLOC: one entry a captured file, counted above.
        let mut seen = std::collections::HashSet::with_capacity(capture.files.len());
        if !capture
            .files
            .iter()
            .all(|(name, _)| seen.insert(name.as_str()))
        {
            return Err(ExportError::Unsupported("a commit that lists a file twice"));
        }
        // ALLOC: one entry a captured file, counted above.
        let lengths: HashMap<&str, u64> = capture
            .lengths
            .iter()
            .map(|(name, length)| (name.as_str(), *length))
            .collect(); // ALLOC: one entry a captured file, counted above.
        // ALLOC: one entry a captured file, at most the manifest's file bound.
        let mut copied: Vec<(ExportedFile, Vec<u8>)> = Vec::with_capacity(capture.files.len());
        for (name, recorded) in &capture.files {
            let location = FileLocation::parse(recorded)?;
            let (key, offset) = from.blob_key(&self.base, name, &location)?;
            let length = lengths
                .get(name.as_str())
                .copied()
                .or_else(|| location.range().map(|(_, length)| length))
                .ok_or(ExportError::Unsupported(
                    "a captured file has no recorded length",
                ))?;
            let wants_head = name.ends_with(".si") || name.starts_with("segments_");
            let mut body = MeasuredSource {
                inner: RangedSource {
                    store: self.store.as_ref(),
                    key: &key,
                    start: offset,
                    length,
                    done: 0,
                    scratch: &mut *buffers.read,
                },
                check: StockFileCheck::measured(length, if wants_head { HEAD_BYTES } else { 0 })
                    .map_err(ExportError::Mismatch)?,
            };
            let blob = format!("__{}", new_id()?); // ALLOC: one blob name a file.
            // ALLOC: one key a file.
            let dest = format!(
                "{}indices/{index_id}/{}/{blob}",
                target.prefix, capture.shard
            );
            match target
                .store
                .create_if_absent(&dest, &mut body, &mut *buffers.write)?
            {
                CreateOutcome::Created => {}
                CreateOutcome::AlreadyExists => {
                    return Err(ExportError::Unsupported(
                        "a new blob name was already taken",
                    ));
                }
            }
            let crc = body.check.finish_crc().map_err(ExportError::Mismatch)?;
            copied.push((
                ExportedFile {
                    name: blob,
                    physical_name: name.clone(), // ALLOC: one name a file.
                    length,
                    checksum: stock_checksum_string(crc),
                    written_by: String::new(),
                },
                body.check.head().to_vec(), // ALLOC: at most HEAD_BYTES, only for .si and segments_N.
            ));
        }
        versions(&mut copied)?;
        // ALLOC: one entry a file, counted above.
        Ok(copied.into_iter().map(|(file, _)| file).collect())
    }
}

/// Fills in each file's `written_by` from the heads kept: a segment's files
/// from its `.si`, a `segments_N` from itself, any other file from the
/// first `segments_N`.
fn versions(copied: &mut [(ExportedFile, Vec<u8>)]) -> Result<(), ExportError> {
    // ALLOC: one entry a segment.
    let mut by_segment: HashMap<String, String> = HashMap::new();
    let mut commit: Option<String> = None;
    for (file, head) in copied.iter() {
        if file.physical_name.ends_with(".si") {
            if let Some(segment) = segment_of(&file.physical_name) {
                let version = lucene_version_of(head, true)?;
                by_segment.insert(segment.to_owned(), version); // ALLOC: one entry a segment.
            }
        } else if file.physical_name.starts_with("segments_") && commit.is_none() {
            commit = Some(lucene_version_of(head, false)?);
        }
    }
    for (file, head) in copied.iter_mut() {
        file.written_by = if file.physical_name.starts_with("segments_") {
            lucene_version_of(head, false)?
        } else if let Some(v) = segment_of(&file.physical_name).and_then(|s| by_segment.get(s)) {
            v.clone() // ALLOC: one short string a file.
        } else {
            commit.clone().ok_or(ExportError::Unsupported(
                "a file with no segment info and no segments file to take a version from",
            ))?
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_blob::Emulator;

    /// A Lucene file of `body` plus a footer with the right CRC.
    fn lucene(body: &[u8]) -> Vec<u8> {
        let mut file = body.to_vec();
        file.extend_from_slice(&[0xC0, 0x28, 0x93, 0xE8, 0, 0, 0, 0]);
        let crc = crc32fast::hash(&file);
        file.extend_from_slice(&u64::from(crc).to_be_bytes());
        file
    }

    fn drain(source: &mut MeasuredSource<'_>, buf: &mut [u8]) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let n = source.fill(buf).unwrap();
            if n == 0 {
                return out;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[test]
    fn a_rewound_source_measures_the_file_again_from_its_start() {
        let store = Emulator::new(1 << 20, 1);
        let bytes = lucene(b"some bytes of a file, longer than one fill");
        store
            .put("k", &mut oss_blob::SliceSource::new(&bytes), &mut [0u8; 8])
            .unwrap();
        let mut scratch = [0u8; 8];
        let mut source = MeasuredSource {
            inner: RangedSource {
                store: &store,
                key: "k",
                start: 0,
                length: bytes.len() as u64,
                done: 0,
                scratch: &mut scratch,
            },
            check: StockFileCheck::measured(bytes.len() as u64, 12).unwrap(),
        };
        let mut buf = [0u8; 8];
        // A first attempt that fails partway, then starts over.
        assert_eq!(source.fill(&mut buf).unwrap(), 8);
        assert_eq!(source.fill(&mut buf).unwrap(), 8);
        source.rewind().unwrap();
        assert_eq!(drain(&mut source, &mut buf), bytes);
        assert_eq!(
            source.check.finish_crc().unwrap(),
            crc32fast::hash(&bytes[..bytes.len() - 8])
        );
        assert_eq!(source.check.head(), &bytes[..12]);
    }

    #[test]
    fn a_segment_is_named_by_its_prefix() {
        assert_eq!(segment_of("_0.cfs"), Some("_0"));
        assert_eq!(segment_of("_1_2.liv"), Some("_1"));
        assert_eq!(segment_of("_a1b.si"), Some("_a1b"));
        assert_eq!(segment_of("segments_4"), None);
        assert_eq!(segment_of("_0"), None);
    }
}
