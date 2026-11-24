//! One shard held by this node: its in-memory directory and the engine
//! over it, owned together so the slot can live in a map.

use lucene_store::ByteBuffersDirectory;
use oss_format::{ShardHead, ShardKey};
use oss_writer::bulk::Kind;
use oss_writer::index::{Expect, Fetched, Refused, Shard, Written};
use oss_writer::mapping::Mapping;
use oss_writer::publish::ShardPublisher;
use oss_writer::takeover::{Adopted, ShardEngine, adopt};

use super::Takeover;
use super::snapshot::Snapshot;

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
    /// Closed by hibernation: the engine's memory is freed and every write
    /// it took is in a published commit.
    asleep: bool,
    /// When a write or a read last reached the shard, in milliseconds.
    used_at: u64,
}

impl ShardSlot {
    /// Opens a shard of `key` under `term`, its documents read against
    /// `mapping`, over the files `fill` puts in its directory first: none
    /// for an empty shard, a commit's for a shard taken over.
    pub(super) fn open(
        key: ShardKey,
        term: i64,
        mapping: Mapping,
        clock: fn() -> u64,
        fill: impl FnOnce(&ByteBuffersDirectory) -> Result<(), String>,
    ) -> Result<Self, String> {
        let owner = ByteBuffersDirectory::new();
        fill(&owner)?;
        let cell = Cell::try_new(owner, |dir| {
            let (w, derived) = oss_writer::index::knn::open_writer(dir, &mapping)?;
            Shard::new(dir, w, mapping, term, Box::new(clock))
                .map(|s| s.with_derived_source(derived))
        })?;
        Ok(Self {
            cell,
            key,
            term,
            released: false,
            asleep: false,
            used_at: super::now_millis(),
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

    /// Whether the slot was put to sleep.
    pub(super) fn asleep(&self) -> bool {
        self.asleep
    }

    /// Marks the slot asleep: a write that finds it so wakes the shard.
    pub(super) fn sleep(&mut self) {
        self.asleep = true;
    }

    /// When a write or a read last reached the shard.
    pub(super) fn used_at(&self) -> u64 {
        self.used_at
    }

    /// Applies one write to the engine.
    pub(super) fn apply(
        &mut self,
        kind: Kind,
        id: &str,
        source: &[u8],
        routing: Option<&str>,
        create: bool,
        expect: &Expect,
    ) -> Result<Written, Refused> {
        self.used_at = super::now_millis();
        self.cell.with_dependent_mut(|_, shard| match kind {
            Kind::Delete => shard.delete(id, expect),
            _ => shard.index_routed(id, source, routing, create, expect),
        })
    }

    /// The highest sequence number the engine assigned, negative for none.
    pub(super) fn max_seq_no(&self) -> i64 {
        self.cell.with_dependent(|_, shard| shard.max_seq_no())
    }

    /// Commits the engine and takes the files of that commit, with the term
    /// to publish them under, so the upload can go on without this slot.
    pub(super) fn snapshot(&mut self) -> Result<(Snapshot, u64), String> {
        let term = u64::try_from(self.term).map_err(|e| e.to_string())?;
        self.cell.with_dependent_mut(|dir, shard| {
            shard.refresh()?;
            let files = shard.committed_files()?;
            Ok((Snapshot::of(dir, &files)?, term))
        })
    }

    /// The live document of `id`; see [`Shard::get`].
    pub(super) fn get(&mut self, id: &str, realtime: bool) -> Result<Option<Fetched>, String> {
        self.used_at = super::now_millis();
        self.cell
            .with_dependent_mut(|_, shard| shard.get(id, realtime))
    }

    /// Brings the shard to a published commit under the head `won`: replays
    /// what the head's writers logged and did not publish, publishes it
    /// through `publisher`, and clears those writers from the head.
    pub(super) fn adopt(
        &mut self,
        publisher: &mut ShardPublisher,
        writer: &str,
        now_millis: u64,
        takeover: &Takeover,
        won: &ShardHead,
    ) -> Result<Adopted, String> {
        let key = self.key.clone();
        self.cell.with_dependent_mut(|dir, shard| {
            let mut engine = ShardEngine::new(shard, dir, publisher, writer, now_millis);
            adopt(
                &key,
                won,
                &takeover.predecessors,
                &mut engine,
                &takeover.heads,
                takeover.max_buffer_bytes,
            )
            .map_err(|e| e.to_string())
        })
    }

    /// Refreshes the engine so its writes are visible to its own reads.
    pub(super) fn refresh(&mut self) -> Result<(), String> {
        self.cell.with_dependent_mut(|_, shard| shard.refresh())
    }
}
