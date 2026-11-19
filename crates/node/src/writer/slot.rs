//! One shard held by this node: its in-memory directory and the engine
//! over it, owned together so the slot can live in a map.

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_format::ShardKey;
use oss_writer::bulk::Kind;
use oss_writer::index::{Expect, Refused, Shard, Written};
use oss_writer::mapping::Mapping;
use oss_writer::publish::{Published, ShardPublisher};

/// The Lucene version the writer's segments are written as.
const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

self_cell::self_cell!(
    /// The directory and the shard that borrows it.
    struct Cell {
        owner: ByteBuffersDirectory,
        #[not_covariant]
        dependent: Shard,
    }
);

/// A shard this node writes, at the term it was opened under.
pub(super) struct ShardSlot {
    cell: Cell,
    key: ShardKey,
    term: i64,
    released: bool,
}

impl ShardSlot {
    /// Opens an empty shard of `key` under `term`, its documents read
    /// against `mapping`.
    pub(super) fn open(
        key: ShardKey,
        term: i64,
        mapping: Mapping,
        clock: fn() -> u64,
    ) -> Result<Self, String> {
        let cell = Cell::try_new(ByteBuffersDirectory::new(), |dir| {
            let mut w = IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION)
                .map_err(|e| e.to_string())?;
            w.enable_explicit_documents().map_err(|e| e.to_string())?;
            Shard::new(dir, w, mapping, term, Box::new(clock))
        })?;
        Ok(Self {
            cell,
            key,
            term,
            released: false,
        })
    }

    /// The shard's identity in the node log.
    pub(super) fn key(&self) -> &ShardKey {
        &self.key
    }

    /// The primary term the shard was opened under.
    pub(super) fn term(&self) -> i64 {
        self.term
    }

    /// Marks the slot released: its engine holds writes the log does not,
    /// so every later write to it is refused.
    pub(super) fn release(&mut self) {
        self.released = true;
    }

    /// Whether the slot was released.
    pub(super) fn released(&self) -> bool {
        self.released
    }

    /// Applies one write to the engine.
    pub(super) fn apply(
        &mut self,
        kind: Kind,
        id: &str,
        source: &[u8],
        create: bool,
        expect: &Expect,
    ) -> Result<Written, Refused> {
        self.cell.with_dependent_mut(|_, shard| match kind {
            Kind::Delete => shard.delete(id, expect),
            _ => shard.index(id, source, create, expect),
        })
    }

    /// The highest sequence number the engine assigned, negative for none.
    pub(super) fn max_seq_no(&self) -> i64 {
        self.cell.with_dependent(|_, shard| shard.max_seq_no())
    }

    /// Commits the engine and publishes that commit through `publisher`.
    pub(super) fn publish(
        &mut self,
        publisher: &mut ShardPublisher,
        writer: &str,
        now_millis: u64,
    ) -> Result<Published, String> {
        let term = u64::try_from(self.term).map_err(|e| e.to_string())?;
        self.cell.with_dependent_mut(|dir, shard| {
            shard.refresh()?;
            let files = shard.committed_files()?;
            publisher
                .publish(dir, &files, term, writer, -1, now_millis)
                .map_err(|e| e.to_string())
        })
    }

    /// Refreshes the engine so its writes are visible to its own reads.
    pub(super) fn refresh(&mut self) -> Result<(), String> {
        self.cell.with_dependent_mut(|_, shard| shard.refresh())
    }
}
