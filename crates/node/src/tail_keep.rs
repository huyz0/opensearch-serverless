//! What a writer keeps of the writes it acknowledged until a commit holds
//! them, so a reader that opens an older commit can be given the rest.
//!
//! Ported from the keeping side of the shell's `TailPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`:
//! `acked`, `published`, `released`, `keptAfter`, `keeps`, `Kept`). Memory
//! only; pushing the records to readers is a separate piece.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use oss_format::WalRecord;

/// How much is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Records kept for one shard.
    pub shard_records: usize,
    /// Bytes kept for one shard.
    pub shard_bytes: u64,
    /// Bytes kept across the node's shards.
    pub node_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            shard_records: 20_000,
            shard_bytes: 16 << 20,
            node_bytes: 256 << 20,
        }
    }
}

#[derive(Default)]
struct Kept {
    /// The sequence number at or below which writes may have been
    /// acknowledged without being kept.
    floor: i64,
    records: VecDeque<WalRecord<'static>>,
    bytes: u64,
    published_through: i64,
    /// The highest sequence number acknowledged and not kept for want of room.
    dropped_through: i64,
}

impl Kept {
    fn new(floor: i64) -> Self {
        Self {
            floor,
            published_through: -1,
            dropped_through: -1,
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct State {
    shards: HashMap<(String, u32), Kept>,
    /// The sum of every shard's bytes.
    bytes: u64,
}

/// The unpublished writes a node acknowledged, per shard.
pub struct TailKeep {
    limits: Limits,
    state: Mutex<State>,
}

impl Default for TailKeep {
    fn default() -> Self {
        Self::new()
    }
}

/// A record counts as the bytes of its id and source.
fn size(record: &WalRecord<'_>) -> u64 {
    (record.id().len() as u64).saturating_add(record.source().len() as u64)
}

impl TailKeep {
    /// A keep with the shell's limits.
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// A keep with other limits.
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes records just acknowledged. With `push` false the shard's keep
    /// begins at them and nothing is stored: readers see them in the next
    /// commit.
    pub fn acked(&self, uuid: &str, shard: u32, records: &[WalRecord<'static>], push: bool) {
        let Some(lowest) = records.iter().map(WalRecord::seq_no).min() else {
            return;
        };
        let mut guard = self.lock();
        let state = &mut *guard;
        let kept = state
            .shards
            .entry((uuid.to_owned(), shard))
            .or_insert_with(|| Kept::new(lowest.saturating_sub(1)));
        if !push {
            return;
        }
        for record in records {
            if record.seq_no() <= kept.published_through {
                continue;
            }
            if state.bytes > self.limits.node_bytes {
                kept.dropped_through = kept.dropped_through.max(record.seq_no());
                continue;
            }
            let bytes = size(record);
            // ALLOC: bounded by the shard's record and byte limits, enforced below
            kept.records.push_back(record.clone());
            kept.bytes = kept.bytes.saturating_add(bytes);
            state.bytes = state.bytes.saturating_add(bytes);
        }
        while kept.records.len() > self.limits.shard_records || kept.bytes > self.limits.shard_bytes
        {
            let Some(dropped) = kept.records.pop_front() else {
                break;
            };
            kept.dropped_through = kept.dropped_through.max(dropped.seq_no());
            let bytes = size(&dropped);
            kept.bytes = kept.bytes.saturating_sub(bytes);
            state.bytes = state.bytes.saturating_sub(bytes);
        }
    }

    /// Drops what a published commit holds: every record at or below its
    /// local checkpoint.
    pub fn published(&self, uuid: &str, shard: u32, checkpoint: i64) {
        let mut guard = self.lock();
        let state = &mut *guard;
        let Some(kept) = state.shards.get_mut(&(uuid.to_owned(), shard)) else {
            return;
        };
        kept.published_through = kept.published_through.max(checkpoint);
        let mut freed = 0_u64;
        kept.records.retain(|record| {
            let held = record.seq_no() <= checkpoint;
            if held {
                freed = freed.saturating_add(size(record));
            }
            !held
        });
        kept.bytes = kept.bytes.saturating_sub(freed);
        state.bytes = state.bytes.saturating_sub(freed);
    }

    /// Forgets a shard this node no longer writes.
    pub fn released(&self, uuid: &str, shard: u32) {
        let mut guard = self.lock();
        if let Some(kept) = guard.shards.remove(&(uuid.to_owned(), shard)) {
            guard.bytes = guard.bytes.saturating_sub(kept.bytes);
        }
    }

    /// The kept records above `after`, oldest first.
    pub fn kept_after(&self, uuid: &str, shard: u32, after: i64) -> Vec<WalRecord<'static>> {
        let guard = self.lock();
        guard
            .shards
            .get(&(uuid.to_owned(), shard))
            .map(|kept| {
                // ALLOC: at most the shard's record limit
                kept.records
                    .iter()
                    .filter(|record| record.seq_no() > after)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The kept records above `after` and whether they are every write
    /// acknowledged above it, read under one lock so a concurrent ack or
    /// publish cannot make the two disagree.
    pub fn snapshot_after(
        &self,
        uuid: &str,
        shard: u32,
        after: i64,
    ) -> (Vec<WalRecord<'static>>, bool) {
        let guard = self.lock();
        let Some(kept) = guard.shards.get(&(uuid.to_owned(), shard)) else {
            return (Vec::new(), false);
        };
        // ALLOC: at most the shard's record limit
        let records = kept
            .records
            .iter()
            .filter(|record| record.seq_no() > after)
            .cloned()
            .collect();
        (records, Self::complete(kept, after))
    }

    fn complete(kept: &Kept, after: i64) -> bool {
        after >= kept.floor && after >= kept.published_through && after >= kept.dropped_through
    }

    /// Whether a reader holding a commit at `after` gets every acknowledged
    /// write above it from [`TailKeep::kept_after`]: false for a shard never
    /// kept, below the floor, below what a publish discarded, and below a record dropped for room.
    pub fn complete_after(&self, uuid: &str, shard: u32, after: i64) -> bool {
        let guard = self.lock();
        guard
            .shards
            .get(&(uuid.to_owned(), shard))
            .is_some_and(|kept| Self::complete(kept, after))
    }

    /// Begins keeping a shard at `floor` unless it is kept already.
    pub fn begin(&self, uuid: &str, shard: u32, floor: i64) {
        self.lock()
            .shards
            .entry((uuid.to_owned(), shard))
            .or_insert_with(|| Kept::new(floor));
    }

    /// Whether a write above `after` was acknowledged and not kept.
    pub fn gap_after(&self, uuid: &str, shard: u32, after: i64) -> bool {
        self.lock()
            .shards
            .get(&(uuid.to_owned(), shard))
            .is_some_and(|kept| kept.dropped_through > after)
    }

    /// Whether anything has begun to be kept for the shard.
    pub fn keeps(&self, uuid: &str, shard: u32) -> bool {
        self.lock().shards.contains_key(&(uuid.to_owned(), shard))
    }

    /// The bytes kept across the node.
    pub fn kept_bytes(&self) -> u64 {
        self.lock().bytes
    }
}
