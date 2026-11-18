//! One in-memory Lucene segment built from the writes a writer pushed but has
//! not published, beside the commit's segments.
//!
//! The records are the latest version of each id above the commit's local
//! checkpoint; an index record is a document, a deletion is none. The
//! documents are the ones the writer would have indexed, built with
//! `oss_doc`. The ids the overlay replaces or deletes are listed so the
//! caller can hide the commit's copies. Nothing here reads the object store.

use std::collections::HashMap;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::{ByteBuffersDirectory, Directory};
use oss_doc::document;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::Mapping;
use oss_format::WalRecord;

/// The most distinct ids one overlay replaces or deletes; more leaves the
/// shard on its commit.
pub const MAX_IDS: usize = 10_000;

/// What the segment's fixed parts take whatever the records: segment
/// metadata, field infos and the writer's own buffers.
const FIXED_BYTES: usize = 256 << 10;
/// What a record's bytes become: postings, doc values, norms and the stored
/// source take a few times its length.
const PER_BYTE: usize = 16;
/// What a record costs beyond its bytes: a document's entries in each
/// metadata field and the segment's per-document tables.
const PER_RECORD: usize = 2048;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

/// Why an overlay could not be built; the caller serves the commit alone.
#[derive(Debug)]
pub enum BuildError {
    /// More than [`MAX_IDS`] distinct ids above the checkpoint.
    TooManyIds,
    /// A record the mapping or the engine could not take.
    Record {
        /// The record's id.
        id: String,
        /// What went wrong.
        reason: String,
    },
    /// The in-memory index could not be written or opened.
    Engine(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyIds => write!(f, "more than {MAX_IDS} ids"),
            Self::Record { id, reason } => write!(f, "record {id}: {reason}"),
            Self::Engine(reason) => write!(f, "in-memory index: {reason}"),
        }
    }
}

impl std::error::Error for BuildError {}

fn engine(e: impl std::fmt::Display) -> BuildError {
    // ALLOC: one message on a failed build.
    BuildError::Engine(e.to_string())
}

/// An upper bound on the heap a build of `records` takes, for the pool charge
/// made before the build.
#[must_use]
pub fn estimate_bytes(records: &[WalRecord<'_>]) -> usize {
    let bytes = records.iter().fold(0_usize, |sum, r| {
        sum.saturating_add(r.id().len())
            .saturating_add(r.source().len())
    });
    FIXED_BYTES
        .saturating_add(bytes.saturating_mul(PER_BYTE))
        .saturating_add(records.len().saturating_mul(PER_RECORD))
}

/// The overlay segment and what it replaces.
pub struct Overlay {
    // Kept so the segment's files outlive the reader over them.
    directory: ByteBuffersDirectory,
    reader: Option<DirectoryReader>,
    ids: Vec<String>,
    doc_count: i32,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("ids", &self.ids.len())
            .field("doc_count", &self.doc_count)
            .finish_non_exhaustive()
    }
}

impl Overlay {
    /// Builds the overlay of `records`, given in ordinal order, above
    /// `local_checkpoint`, read against `mapping`.
    ///
    /// # Errors
    ///
    /// [`BuildError::TooManyIds`] past [`MAX_IDS`] distinct ids,
    /// [`BuildError::Record`] for a source the mapping refuses, and
    /// [`BuildError::Engine`] when the in-memory index fails.
    pub fn build(
        mapping: &Mapping,
        records: &[WalRecord<'_>],
        local_checkpoint: i64,
    ) -> Result<Self, BuildError> {
        let (kept, ids) = latest(records, local_checkpoint)?;
        let directory = ByteBuffersDirectory::new();
        let mut doc_count = 0_i32;
        {
            // ALLOC: one index writer a build, bounded by the pool charge made from estimate_bytes.
            let mut writer =
                IndexWriter::open(&directory, Vec::new(), "Lucene104", VERSION).map_err(engine)?;
            writer.enable_explicit_documents().map_err(engine)?;
            let mut indexer = Indexer::new();
            // ALLOC: one document a kept index record, at most MAX_IDS.
            let mut docs = Vec::new();
            for r in kept.iter().filter(|r| !r.is_deletion()) {
                // ALLOC: one id and message on a failed build.
                let refuse = |reason: String| BuildError::Record {
                    id: r.id().to_owned(),
                    reason,
                };
                let parsed = document::parse(mapping, r.id(), r.source().as_bytes())
                    // ALLOC: one message on a failed build.
                    .map_err(|e| refuse(format!("{e:?}")))?;
                let meta = Meta {
                    id: r.id(),
                    seq_no: r.seq_no(),
                    primary_term: r.primary_term(),
                    version: r.version(),
                    source: r.source().as_bytes(),
                };
                let doc = indexer
                    .document(&mut writer, mapping, &parsed, &meta)
                    // ALLOC: one message on a failed build.
                    .map_err(|e| refuse(format!("{e:?}")))?;
                docs.push(doc);
                doc_count = doc_count.saturating_add(1);
            }
            if !docs.is_empty() {
                writer.add_explicit_documents(docs).map_err(engine)?;
                writer.commit().map_err(engine)?;
            }
        }
        // No documents wrote no commit; such an overlay has no reader.
        let reader = if doc_count == 0 {
            None
        } else {
            Some(DirectoryReader::open(&directory).map_err(engine)?)
        };
        Ok(Self {
            directory,
            reader,
            ids,
            doc_count,
        })
    }

    /// The segment's reader; `None` when the overlay holds no document.
    #[must_use]
    pub fn reader(&self) -> Option<&DirectoryReader> {
        self.reader.as_ref()
    }

    /// Every id the overlay replaces or deletes, once, in first-seen order.
    #[must_use]
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// How many documents the overlay holds.
    #[must_use]
    pub fn doc_count(&self) -> i32 {
        self.doc_count
    }

    /// The bytes of the segment's files, what [`estimate_bytes`] must cover.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.directory
            .list_all()
            .unwrap_or_default()
            .iter()
            .filter_map(|name| self.directory.file_length(name).ok())
            .fold(0_usize, |sum, n| {
                sum.saturating_add(usize::try_from(n).unwrap_or(usize::MAX))
            })
    }
}

/// The record that wins for each id above `checkpoint`, in the order its id
/// was first seen, and those ids.
fn latest<'r, 'a>(
    records: &'r [WalRecord<'a>],
    checkpoint: i64,
) -> Result<(Vec<&'r WalRecord<'a>>, Vec<String>), BuildError> {
    // ALLOC: one entry an id, at most MAX_IDS.
    let mut slot: HashMap<&str, usize> = HashMap::new();
    // ALLOC: one entry an id, at most MAX_IDS.
    let mut kept: Vec<&WalRecord<'_>> = Vec::new();
    for r in records.iter().filter(|r| r.seq_no() > checkpoint) {
        match slot.get(r.id()) {
            Some(&i) => {
                if let Some(held) = kept.get_mut(i).filter(|held| r.seq_no() >= held.seq_no()) {
                    *held = r;
                }
            }
            None => {
                if kept.len() >= MAX_IDS {
                    return Err(BuildError::TooManyIds);
                }
                slot.insert(r.id(), kept.len());
                kept.push(r);
            }
        }
    }
    // ALLOC: one string an id, at most MAX_IDS.
    let ids = kept.iter().map(|r| r.id().to_owned()).collect();
    Ok((kept, ids))
}
