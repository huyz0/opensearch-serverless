//! One shard of the writer as `InternalEngine` runs a primary's index and
//! delete: the version lookup, the plan, a sequence number for each write
//! that goes ahead, and the Lucene update or tombstone it makes.

use std::sync::Arc;

use lucene_index::buffered_updates::Term;
use lucene_index::index_writer::IndexWriter;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::Directory;
use oss_ext::analyzer::Analyze;

use crate::document;
use crate::fast::FastIngest;
use crate::lucene::{Indexer, Meta, uid};
use crate::mapping::{Mapping, MappingError};

mod commit;
mod get;
mod growth;
mod merge;
mod replay;
mod soft;

pub use get::Fetched;
pub use merge::{MergeGate, MergeGuard, MergeSettings};
use soft::soft_delete;

use super::lookup;
use super::plan::{Current, Expect, plan};
use super::versions::{GC_DELETES_MILLIS, VersionMap};

/// `SequenceNumbers.NO_OPS_PERFORMED`.
pub const NO_OPS_PERFORMED: i64 = -1;

/// `SequenceNumbers.MAX_SEQ_NO`: the commit user data key of the highest
/// sequence number a commit holds.
pub const MAX_SEQ_NO_KEY: &str = "max_seq_no";

/// `SequenceNumbers.LOCAL_CHECKPOINT_KEY`.
pub const LOCAL_CHECKPOINT_KEY: &str = "local_checkpoint";

/// `Translog.TRANSLOG_UUID_KEY`: the translog a commit belongs to, which
/// `InternalEngine`'s constructor requires of the commit it opens.
pub const TRANSLOG_UUID_KEY: &str = "translog_uuid";

/// `Engine.HISTORY_UUID_KEY`.
pub const HISTORY_UUID_KEY: &str = "history_uuid";

/// What a write did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An index of an id with no live document.
    Created,
    /// An index that replaced a live document.
    Updated,
    /// A delete of a live document.
    Deleted,
    /// A delete of an id with no live document: it still writes a
    /// tombstone and takes a version and sequence number.
    NotFound,
}

impl Outcome {
    /// The `result` the documents API shows.
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Created => "created",
            Outcome::Updated => "updated",
            Outcome::Deleted => "deleted",
            Outcome::NotFound => "not_found",
        }
    }
}

/// A write that went ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    /// What it did.
    pub outcome: Outcome,
    /// The version it gave the id.
    pub version: i64,
    /// Its sequence number.
    pub seq_no: i64,
    /// The primary term it was written under.
    pub primary_term: i64,
}

/// A write that did not go ahead, and took no sequence number.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// `version_conflict_engine_exception`, with its message.
    Conflict(String),
    /// The document, the mapping or the engine refused it.
    Mapping(MappingError),
}

/// One shard: its index writer, the mapping documents are read against,
/// and the sequence numbers and versions it has given.
pub struct Shard<'d> {
    dir: &'d dyn Directory,
    writer: IndexWriter<'d>,
    indexer: Indexer,
    fast: FastIngest,
    mapping: Arc<Mapping>,
    growth: Option<Mapping>,
    versions: VersionMap,
    reader: Option<DirectoryReader>,
    merging: Option<merge::Merging>,
    merge_failures: u64,
    primary_term: i64,
    max_seq_no: i64,
    last_write_millis: u64,
    clock: Box<dyn Fn() -> u64 + Send + 'd>,
}

impl<'d> Shard<'d> {
    /// A shard over `writer`, which writes to `dir`, its documents read
    /// against `mapping` and written under `primary_term`; `clock` gives
    /// milliseconds, for keeping deletes `gc_deletes` long. Over an index
    /// already committed, sequence numbers continue from the commit's
    /// `max_seq_no` (`loadSeqNoInfoFromLuceneCommit`).
    ///
    /// # Errors
    ///
    /// The segments already in `dir` not opening, or a commit whose
    /// `max_seq_no` does not read.
    pub fn new(
        dir: &'d dyn Directory,
        writer: IndexWriter<'d>,
        mapping: impl Into<Arc<Mapping>>,
        primary_term: i64,
        clock: Box<dyn Fn() -> u64 + Send + 'd>,
    ) -> Result<Self, String> {
        let max_seq_no = match writer
            .live_commit_data()
            .iter()
            .find(|(k, _)| k == MAX_SEQ_NO_KEY)
        {
            Some((_, v)) => v.parse::<i64>().map_err(|e| message(&e))?,
            None => NO_OPS_PERFORMED,
        };
        let opened = clock();
        let mut shard = Shard {
            dir,
            writer,
            indexer: Indexer::new(),
            fast: FastIngest::new(),
            mapping: mapping.into(),
            growth: None,
            versions: VersionMap::default(),
            reader: None,
            merging: None,
            merge_failures: 0,
            primary_term,
            max_seq_no,
            last_write_millis: opened,
            clock,
        };
        shard.open_reader()?;
        Ok(shard)
    }

    /// Stores each document's `_source` with its vectors masked when `on`:
    /// for a writer [`knn::open_writer`](crate::index::knn::open_writer) opened as derived.
    #[must_use]
    pub fn with_derived_source(mut self, on: bool) -> Self {
        self.indexer = std::mem::take(&mut self.indexer).with_derived_source(on);
        self
    }

    /// Indexes the fields whose mapping names `name` as their analyzer with
    /// `module`, the module a pin loaded, never with the built-in analyzer.
    #[must_use]
    pub fn with_module(mut self, name: &str, module: Arc<dyn Analyze>) -> Self {
        self.indexer = std::mem::take(&mut self.indexer).with_module(name, module);
        self
    }

    /// The mapping, grown by the dynamic updates documents made.
    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// The time since the shard's last index or delete, or since it opened:
    /// what the inactive-shard check reads.
    pub fn idle_millis(&self) -> u64 {
        (self.clock)().saturating_sub(self.last_write_millis)
    }

    /// The highest sequence number given, the local checkpoint: a primary
    /// gives them in order and each write completes before the next.
    pub fn max_seq_no(&self) -> i64 {
        self.max_seq_no
    }

    fn open_reader(&mut self) -> Result<(), String> {
        // An index with no commit yet has no segments to look in.
        if self.writer.commit_generations().is_empty() {
            self.reader = None;
            return Ok(());
        }
        let reader = DirectoryReader::open(self.dir).map_err(|e| message(&e))?;
        self.reader = Some(
            reader
                .soft_deletes_wrapper("__soft_deletes")
                .map_err(|e| message(&e))?,
        );
        Ok(())
    }

    /// The heap the version map holds, which counts toward the indexing
    /// buffer with the writer's own (`IndexingMemoryController` refreshes a
    /// shard when the buffer fills).
    pub fn version_map_bytes(&self) -> usize {
        self.versions.ram_bytes()
    }

    /// The heap the shard's indexing buffer holds: the writer's buffered
    /// documents and the version map. A caller refreshes the shard when it
    /// passes the node's indexing buffer.
    pub fn indexing_buffer_bytes(&self) -> usize {
        self.writer
            .ram_bytes_used()
            .saturating_add(self.versions.ram_bytes())
    }

    fn current(&self, id: &str, uid: &[u8]) -> Result<Option<Current>, Refused> {
        if let Some(c) = self.versions.get(id, (self.clock)(), GC_DELETES_MILLIS) {
            return Ok(Some(c));
        }
        match &self.reader {
            Some(r) => {
                lookup::find(r, uid).map_err(|e| Refused::Mapping(MappingError::illegal_state(&e)))
            }
            None => Ok(None),
        }
    }

    fn next_seq_no(&mut self) -> i64 {
        self.max_seq_no = self.max_seq_no.saturating_add(1);
        self.max_seq_no
    }

    fn engine(e: &dyn std::fmt::Display) -> Refused {
        Refused::Mapping(MappingError::illegal_state(&message(e)))
    }

    /// Indexes `source` as `id`: `create` for `op_type=create`.
    ///
    /// # Errors
    ///
    /// A version conflict, or the document refused; neither takes a
    /// sequence number.
    pub fn index(
        &mut self,
        id: &str,
        source: &[u8],
        create: bool,
        expect: &Expect,
    ) -> Result<Written, Refused> {
        self.index_routed(id, source, None, create, expect)
    }

    /// As [`Shard::index`], keeping the custom `routing` value so a get
    /// returns it. A routed document takes the complete path.
    ///
    /// # Errors
    ///
    /// As [`Shard::index`].
    pub fn index_routed(
        &mut self,
        id: &str,
        source: &[u8],
        routing: Option<&str>,
        create: bool,
        expect: &Expect,
    ) -> Result<Written, Refused> {
        let expect = if create {
            Expect {
                version: super::version::MATCH_DELETED,
                version_type: super::version::VersionType::Internal,
                ..*expect
            }
        } else {
            *expect
        };
        // The plan reads the source when it can; whatever it declines, and every error, is the
        // complete path's. An empty id has no key, and takes the complete path's error order.
        let fast = uid(id)
            .ok()
            .filter(|_| routing.is_none())
            .and_then(|k| self.fast.prepare(&self.indexer, &self.mapping, source, &k));
        let parsed = match fast {
            Some(_) => None,
            None => Some(document::parse(&self.mapping, id, source).map_err(Refused::Mapping)?),
        };
        let key = uid(id).map_err(Refused::Mapping)?;
        // The Java publishes a document's mapping update before the engine
        // plans it, so the update holds even when the write conflicts.
        if let Some(grown) = parsed.as_ref().and_then(|p| p.grown.as_ref()) {
            // ALLOC: the grown mapping, bounded by the total fields limit; this shard's own.
            self.mapping = Arc::new(grown.clone());
            self.fast.invalidate();
            self.note_growth(parsed.as_ref().and_then(|p| p.update.as_ref()));
        }
        let current = self.current(id, &key)?;
        let p = plan(id, current, &expect).map_err(Refused::Conflict)?;
        let mapping = &self.mapping;
        let seq_no = self.max_seq_no.saturating_add(1);
        let meta = Meta {
            id,
            seq_no,
            primary_term: self.primary_term,
            version: p.version,
            source,
        };
        let soft = if p.not_found_or_deleted {
            None
        } else {
            Some(
                self.indexer
                    .soft_deletes(&mut self.writer)
                    .map_err(Refused::Mapping)?,
            )
        };
        // The documents of one write: a nested object's child documents first, the root last.
        let (docs, vectors) = match (fast, &parsed) {
            (Some(f), _) => (
                f.finish(seq_no, self.primary_term, p.version),
                // ALLOC: no vectors.
                Vec::new(),
            ),
            (None, Some(parsed)) => {
                let (docs, vectors) = self
                    .indexer
                    .block_with_vectors(&mut self.writer, mapping, parsed, &meta, routing)
                    .map_err(Refused::Mapping)?;
                (docs, vectors)
            }
            (None, None) => {
                return Err(Refused::Mapping(MappingError::illegal_state(
                    "no document read",
                )));
            }
        };
        if docs.is_empty() {
            return Err(Refused::Mapping(MappingError::illegal_state(
                "no document read",
            )));
        }
        let has_vectors = vectors.iter().any(|v| !v.is_empty());
        match (soft, has_vectors) {
            (None, false) => self.writer.add_explicit_documents(docs),
            (None, true) => self
                .writer
                .add_explicit_documents_with_vectors(None, docs, vectors),
            (Some(_), false) => self.writer.soft_update_explicit_documents(
                Term::new("_id", key.clone()),
                docs,
                &[soft_delete(&key)],
            ),
            (Some(_), true) => self.writer.soft_update_explicit_documents_with_vectors(
                Term::new("_id", key.clone()),
                docs,
                vectors,
                &[soft_delete(&key)],
            ),
        }
        .map_err(|e| Self::engine(&e))?;
        self.next_seq_no();
        let current = Current {
            version: p.version,
            seq_no,
            term: self.primary_term,
            deleted: false,
        };
        let now = (self.clock)();
        self.versions.put(id, current, now);
        self.last_write_millis = now;
        Ok(Written {
            outcome: if p.not_found_or_deleted {
                Outcome::Created
            } else {
                Outcome::Updated
            },
            version: p.version,
            seq_no,
            primary_term: self.primary_term,
        })
    }

    /// Deletes `id`, writing a tombstone whether or not it holds a live
    /// document.
    ///
    /// # Errors
    ///
    /// A version conflict, which takes no sequence number.
    pub fn delete(&mut self, id: &str, expect: &Expect) -> Result<Written, Refused> {
        let key = uid(id).map_err(Refused::Mapping)?;
        let current = self.current(id, &key)?;
        let p = plan(id, current, expect).map_err(Refused::Conflict)?;
        let seq_no = self.max_seq_no.saturating_add(1);
        if !p.not_found_or_deleted {
            self.indexer
                .soft_deletes(&mut self.writer)
                .map_err(Refused::Mapping)?;
        }
        let tombstone = self
            .indexer
            .tombstone(&mut self.writer, id, seq_no, self.primary_term, p.version)
            .map_err(Refused::Mapping)?;
        // ALLOC: one tombstone a delete.
        let docs = vec![tombstone];
        if p.not_found_or_deleted {
            self.writer.add_explicit_documents(docs)
        } else {
            self.writer.soft_update_explicit_documents(
                Term::new("_id", key.clone()),
                docs,
                &[soft_delete(&key)],
            )
        }
        .map_err(|e| Self::engine(&e))?;
        self.next_seq_no();
        let current = Current {
            version: p.version,
            seq_no,
            term: self.primary_term,
            deleted: true,
        };
        let now = (self.clock)();
        self.versions.put(id, current, now);
        self.last_write_millis = now;
        Ok(Written {
            outcome: if p.not_found_or_deleted {
                Outcome::NotFound
            } else {
                Outcome::Deleted
            },
            version: p.version,
            seq_no,
            primary_term: self.primary_term,
        })
    }
}

/// The engine's error as a message.
fn message(e: &dyn std::fmt::Display) -> String {
    // ALLOC: error path; one message.
    e.to_string()
}
