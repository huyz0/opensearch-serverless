//! One in-memory Lucene segment built from the writes a writer pushed but has
//! not published, beside the commit's segments.
//!
//! The records are the latest version of each id above the commit's local
//! checkpoint; an index record is a document, a deletion is none. The
//! documents are the ones the writer would have indexed, built with
//! `oss_doc`. The ids the overlay replaces or deletes are listed so the
//! caller can hide the commit's copies. Nothing here reads the object store.

mod commit;
mod source;
mod view;

use std::collections::HashMap;
use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::Mapping;
use oss_ext::analyzer::Analyze;
use oss_format::WalRecord;

pub use commit::{Copies, Copy, find_copies, local_checkpoint, mask_bytes};
pub use source::{CommitAccess, OverlaySource, Ready, build_ready, build_ready_with};
pub use view::{Masks, combine};

/// The most distinct ids one overlay replaces or deletes; more leaves the
/// shard on its commit.
pub const MAX_IDS: usize = 10_000;

/// What the segment's fixed parts take whatever the records: segment
/// metadata, field infos and the writer's own buffers.
const FIXED_BYTES: usize = 256 << 10;
/// What a finished overlay keeps whatever the records.
const KEPT_FIXED_BYTES: usize = 64 << 10;
/// What a finished overlay keeps for each byte of its segment files.
const KEPT_PER_STORED: usize = 3;
/// A finished overlay keeps this part of the source it decoded.
const KEPT_SOURCE_DIVISOR: usize = 4;
/// What a record's bytes become at the build's peak: the parsed document, the
/// writer's buffers and the stored source take a few times its length
/// (measured by `tests/overlay_heap.rs`, about twice).
const PER_BYTE: usize = 4;
/// What a record costs beyond its bytes at the build's peak: a document's
/// entries in each field and the segment's per-document tables (measured
/// about 3 KiB).
const PER_RECORD: usize = 4096;

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
    /// The records kept would take more than the bytes the caller reserved.
    TooManyBytes {
        /// What [`estimate_bytes`] gives for the kept records.
        estimate: usize,
        /// What the caller reserved.
        max_bytes: usize,
    },
    /// The in-memory index could not be written or opened.
    Engine(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyIds => write!(f, "more than {MAX_IDS} ids"),
            Self::Record { id, reason } => write!(f, "record {id}: {reason}"),
            Self::TooManyBytes {
                estimate,
                max_bytes,
            } => write!(f, "{estimate} bytes over the {max_bytes} reserved"),
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
    estimate_of(records.iter())
}

/// What a finished overlay keeps, for the pool charge to fall to once the
/// build's peak is over: the segment files it holds (`stored`), the readers
/// over them and the source it decoded, measured by `tests/overlay_heap.rs`
/// against the heap the finished overlay retains, over small and large
/// records, compressible and not, and a wide mapping. Never more than the
/// build's estimate.
#[must_use]
pub fn kept_bytes(records: &[WalRecord<'_>], stored: usize) -> usize {
    let source = records.iter().fold(0_usize, |bytes, r| {
        bytes
            .saturating_add(r.id().len())
            .saturating_add(r.source().len())
    });
    KEPT_FIXED_BYTES
        .saturating_add(stored.saturating_mul(KEPT_PER_STORED))
        .saturating_add(source / KEPT_SOURCE_DIVISOR)
        .min(estimate_bytes(records))
}

fn estimate_kept(kept: &[&WalRecord<'_>]) -> usize {
    estimate_of(kept.iter().copied())
}

fn estimate_of<'r, 'a: 'r>(records: impl Iterator<Item = &'r WalRecord<'a>>) -> usize {
    let (count, bytes) = records.fold((0_usize, 0_usize), |(count, bytes), r| {
        (
            count.saturating_add(1),
            bytes
                .saturating_add(r.id().len())
                .saturating_add(r.source().len()),
        )
    });
    FIXED_BYTES
        .saturating_add(bytes.saturating_mul(PER_BYTE))
        .saturating_add(count.saturating_mul(PER_RECORD))
}

/// The overlay segment and what it replaces.
pub struct Overlay {
    // Kept so the segment's files outlive the reader over them.
    _directory: ByteBuffersDirectory,
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
    /// `local_checkpoint`, read against `mapping`, taking no more than
    /// `max_bytes` by [`estimate_bytes`] of the records kept (the bytes the
    /// caller reserved from the pool before calling).
    ///
    /// # Errors
    ///
    /// [`BuildError::TooManyIds`] past [`MAX_IDS`] distinct ids,
    /// [`BuildError::TooManyBytes`] when the kept records need more than
    /// `max_bytes`,
    /// [`BuildError::Record`] for a source the mapping refuses, and
    /// [`BuildError::Engine`] when the in-memory index fails.
    pub fn build(
        mapping: &Mapping,
        records: &[WalRecord<'_>],
        local_checkpoint: i64,
        max_bytes: usize,
    ) -> Result<Self, BuildError> {
        Self::build_over(
            mapping,
            records,
            local_checkpoint,
            max_bytes,
            &Copies::default(),
        )
    }

    /// As [`Self::build`], dropping each record whose copy in the commit has
    /// a sequence number at or above its own: the commit already holds that
    /// version or a newer one.
    ///
    /// # Errors
    ///
    /// As [`Self::build`].
    pub fn build_over(
        mapping: &Mapping,
        records: &[WalRecord<'_>],
        local_checkpoint: i64,
        max_bytes: usize,
        copies: &Copies,
    ) -> Result<Self, BuildError> {
        Self::build_over_with(mapping, records, local_checkpoint, max_bytes, copies, &[])
    }

    /// As [`Self::build_over`], indexing the fields that pin an analyzer
    /// with the module `modules` names.
    ///
    /// # Errors
    ///
    /// As [`Self::build`].
    pub fn build_over_with(
        mapping: &Mapping,
        records: &[WalRecord<'_>],
        local_checkpoint: i64,
        max_bytes: usize,
        copies: &Copies,
        modules: &[(String, Arc<dyn Analyze>)],
    ) -> Result<Self, BuildError> {
        // Nothing is allocated for the index until both caps have passed.
        let (kept, ids) = latest(records, local_checkpoint, copies)?;
        let estimate = estimate_kept(&kept);
        if estimate > max_bytes {
            return Err(BuildError::TooManyBytes {
                estimate,
                max_bytes,
            });
        }
        let directory = ByteBuffersDirectory::new();
        let mut doc_count = 0_i32;
        {
            // ALLOC: one index writer a build, bounded by the pool charge made from estimate_bytes.
            let mut writer =
                IndexWriter::open(&directory, Vec::new(), "Lucene104", VERSION).map_err(engine)?;
            writer.enable_explicit_documents().map_err(engine)?;
            let mut indexer = Indexer::new();
            for (name, module) in modules {
                indexer = indexer.with_module(name, Arc::clone(module));
            }
            let mut fast = FastIngest::new();
            // ALLOC: one document a kept index record, at most MAX_IDS.
            let mut docs = Vec::new();
            // The mapping as dynamic mapping has grown it over the records so far.
            let mut grown: Option<Mapping> = None;
            for r in kept.iter().filter(|r| !r.is_deletion()) {
                // ALLOC: one id and message on a failed build.
                let refuse = |reason: String| BuildError::Record {
                    id: r.id().to_owned(),
                    reason,
                };
                let source = r.source().as_bytes();
                // The plan reads what it can; whatever it declines is the complete path's.
                let current = grown.as_ref().unwrap_or(mapping);
                let read = oss_doc::lucene::uid(r.id())
                    .ok()
                    .and_then(|key| fast.prepare(&indexer, current, source, &key));
                let doc = match read {
                    Some(f) => f
                        .finish(r.seq_no(), r.primary_term(), r.version())
                        .pop()
                        // ALLOC: one message on a failed build.
                        .ok_or_else(|| refuse("no document read".to_owned()))?,
                    None => {
                        let parsed = document::parse(current, r.id(), source)
                            // ALLOC: one message on a failed build.
                            .map_err(|e| refuse(format!("{e:?}")))?;
                        let meta = Meta {
                            id: r.id(),
                            seq_no: r.seq_no(),
                            primary_term: r.primary_term(),
                            version: r.version(),
                            source,
                        };
                        let mut parsed = parsed;
                        let doc = indexer
                            .document(
                                &mut writer,
                                parsed.grown.as_ref().unwrap_or(current),
                                &parsed,
                                &meta,
                            )
                            // ALLOC: one message on a failed build.
                            .map_err(|e| refuse(format!("{e:?}")))?;
                        if let Some(next) = parsed.grown.take() {
                            grown = Some(next);
                        }
                        doc
                    }
                };
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
            _directory: directory,
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

    /// The commit documents the overlay hides, as `(segment, document)`: the
    /// copy of every id it replaces or deletes that the commit holds.
    #[must_use]
    pub fn hidden(&self, copies: &Copies) -> Vec<(usize, i32)> {
        self.ids
            .iter()
            .filter_map(|id| copies.get(id))
            .map(|c| (c.segment, c.doc))
            // ALLOC: one entry an id, at most MAX_IDS.
            .collect()
    }

    /// The bytes of the overlay's segment files, which its in-memory
    /// directory holds.
    #[must_use]
    pub fn stored_bytes(&self) -> usize {
        use lucene_store::Directory;
        let names = self._directory.list_all().unwrap_or_default();
        names.iter().fold(0_usize, |bytes, name| {
            let len = self._directory.file_length(name).unwrap_or(0);
            bytes.saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
        })
    }

    /// The documents the overlay holds.
    #[must_use]
    pub fn doc_count(&self) -> i32 {
        self.doc_count
    }
}

/// The record that wins for each id above `checkpoint`, in the order its id
/// was first seen, and those ids.
fn latest<'r, 'a>(
    records: &'r [WalRecord<'a>],
    checkpoint: i64,
    copies: &Copies,
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
    kept.retain(|r| copies.get(r.id()).is_none_or(|c| r.seq_no() > c.seq_no));
    // ALLOC: one string an id, at most MAX_IDS.
    let ids = kept.iter().map(|r| r.id().to_owned()).collect();
    Ok((kept, ids))
}

/// The distinct ids with a record above `local_checkpoint`, in first-seen
/// order: the ids to look up in the commit before building.
///
/// # Errors
///
/// [`BuildError::TooManyIds`] past [`MAX_IDS`] ids.
pub fn candidate_ids(
    records: &[WalRecord<'_>],
    local_checkpoint: i64,
) -> Result<Vec<String>, BuildError> {
    latest(records, local_checkpoint, &Copies::default()).map(|(_, ids)| ids)
}
