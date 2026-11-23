//! The writes a writer pushed a reader, held until an engine applies them on
//! top of the commit it opened.
//!
//! Ported from the receiving side of the shell's `TailOverlays`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailOverlays.java`:
//! `handle`, `take`, `track`, `untrack`, `skipped`, `nextOrdinal`, `from`,
//! `answered`, `granted`). Memory only, and it holds no clock: callers pass
//! the time in milliseconds. The engine, the fetch and the transport handler
//! are separate pieces.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use oss_transport::actions::tail::{
    CHECK, FetchResponse, NEW, NONE, PushRequest, PushResponse, Record,
};

/// Records held for one shard: the shell's `MAX_DOCUMENTS * 2`.
pub const MAX_RECORDS: usize = 100_000;

/// How long a shard's records are kept after its last engine closed, for the
/// one about to open.
pub const KEEP_AFTER_CLOSE_MILLIS: i64 = 30_000;

/// How long a grant lasts from the moment its fetch was sent.
pub const GRANT_MILLIS: i64 = 500;

struct Held {
    records: BTreeMap<u64, Record>,
    next_ordinal: u64,
    engines: u32,
    local_checkpoint: i64,
    released_at: i64,
    /// A record did not fit: later ones would leave a gap, so none is taken
    /// until an engine opens anew.
    refusing: bool,
    /// An engine left a record out for a reason other than holding a newer
    /// write; cleared by a new engine.
    skipped: bool,
    grant_writer: Option<String>,
    /// The subscription the grant is under, or [`NONE`].
    subscription: i64,
    /// Whether what the grant's fetch answered, with the commit, held every
    /// write its writer acknowledged.
    complete: bool,
    grant_until: i64,
}

impl Held {
    fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            next_ordinal: 0,
            engines: 0,
            local_checkpoint: -1,
            released_at: 0,
            refusing: false,
            skipped: false,
            grant_writer: None,
            subscription: NONE,
            complete: false,
            grant_until: 0,
        }
    }

    /// Whether nobody reopened the shard within the keep.
    fn expired(&self, now: i64) -> bool {
        self.engines == 0 && now.saturating_sub(self.released_at) > KEEP_AFTER_CLOSE_MILLIS
    }

    /// Stores what is above the checkpoint; `None` when unwanted.
    fn take(&mut self, records: &[Record]) -> Option<usize> {
        if self.refusing {
            return None;
        }
        let mut taken = 0_usize;
        for record in records {
            if record.seq_no <= self.local_checkpoint {
                continue;
            }
            if self.records.len() >= MAX_RECORDS {
                // Full: the writer stops pushing, and the reader serves what
                // it holds until it opens a newer commit.
                self.refusing = true;
                return None;
            }
            // ALLOC: at most MAX_RECORDS a shard, enforced above
            self.records.insert(self.next_ordinal, record.clone());
            self.next_ordinal = self.next_ordinal.saturating_add(1);
            taken = taken.saturating_add(1);
        }
        Some(taken)
    }
}

/// What the node holds of its shards' pushed writes.
#[derive(Default)]
pub struct TailHeld {
    shards: Mutex<HashMap<(String, u32), Held>>,
}

fn key(uuid: &str, shard: u32) -> (String, u32) {
    (uuid.to_owned(), shard)
}

impl TailHeld {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(String, u32), Held>> {
        self.shards.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many shards are held.
    pub fn held_shards(&self) -> usize {
        self.lock().len()
    }

    /// Takes pushed records: how many were stored, or `None` when the shard
    /// is unwanted (no engine tracks it, it ran out of room, or it was let
    /// go).
    pub fn take(&self, uuid: &str, shard: u32, records: &[Record], now: i64) -> Option<usize> {
        let mut shards = self.lock();
        let held = shards.get_mut(&key(uuid, shard))?;
        if held.expired(now) {
            // Let go: nobody reopened it in time.
            shards.remove(&key(uuid, shard));
            return None;
        }
        held.take(records)
    }

    /// An engine opened on a commit at `local_checkpoint`.
    pub fn track(&self, uuid: &str, shard: u32, local_checkpoint: i64) {
        let mut shards = self.lock();
        let held = shards.entry(key(uuid, shard)).or_insert_with(Held::new);
        held.engines = held.engines.saturating_add(1);
        held.local_checkpoint = held.local_checkpoint.max(local_checkpoint);
        // A new engine fetches what follows its commit, so what was refused
        // is asked for again, and what an earlier engine left out is tried
        // again.
        held.refusing = false;
        held.skipped = false;
        let checkpoint = held.local_checkpoint;
        held.records.retain(|_, record| record.seq_no > checkpoint);
    }

    /// An engine closed. Shards nobody reopened are let go of here too.
    pub fn untrack(&self, uuid: &str, shard: u32, now: i64) {
        let mut shards = self.lock();
        if let Some(held) = shards.get_mut(&key(uuid, shard)) {
            held.engines = held.engines.saturating_sub(1);
            if held.engines == 0 {
                held.released_at = now;
            }
        }
        shards.retain(|_, held| !held.expired(now));
    }

    /// An engine left a record out of what it searches.
    pub fn skipped(&self, uuid: &str, shard: u32) {
        if let Some(held) = self.lock().get_mut(&key(uuid, shard)) {
            held.skipped = true;
        }
    }

    /// The ordinal the next record held takes.
    pub fn next_ordinal(&self, uuid: &str, shard: u32) -> u64 {
        self.lock()
            .get(&key(uuid, shard))
            .map_or(0, |held| held.next_ordinal)
    }

    /// The records held at or after an ordinal, in order.
    pub fn from(&self, uuid: &str, shard: u32, ordinal: u64) -> Vec<(u64, Record)> {
        self.lock()
            .get(&key(uuid, shard))
            .map(|held| {
                // ALLOC: at most MAX_RECORDS
                held.records
                    .range(ordinal..)
                    .map(|(ordinal, record)| (*ordinal, record.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Takes a fetch's answer: its records, and its grant. `asked` is the
    /// subscription the fetch named and `sent_at` when it was sent.
    pub fn answered(
        &self,
        uuid: &str,
        shard: u32,
        writer: &str,
        asked: i64,
        sent_at: i64,
        answer: &FetchResponse,
    ) {
        let taken = self.take(uuid, shard, &answer.records, sent_at);
        let mut shards = self.lock();
        let Some(held) = shards.get_mut(&key(uuid, shard)) else {
            return;
        };
        if answer.subscription == NONE {
            if held.grant_writer.as_deref() == Some(writer) {
                held.grant_writer = None;
                held.subscription = NONE;
                held.complete = false;
            }
            return;
        }
        let renewed = asked >= 0
            && answer.subscription == asked
            && held.grant_writer.as_deref() == Some(writer);
        if !renewed {
            // A subscription anew: the records it answered, with the commit,
            // are all there is to go on.
            held.grant_writer = Some(writer.to_owned());
            held.subscription = answer.subscription;
            held.complete = answer.complete && taken.is_some();
            held.grant_until = sent_at;
        }
        let until = sent_at.saturating_add(GRANT_MILLIS);
        held.grant_until = held.grant_until.max(until);
    }

    /// What to ask `writer` for a grant: the subscription held to renew, a
    /// new one, or only how far its refreshing writes went ([`CHECK`]) when
    /// no engine could use a subscription.
    pub fn subscription_to_ask(&self, uuid: &str, shard: u32, writer: &str) -> i64 {
        self.lock()
            .get(&key(uuid, shard))
            .filter(|held| held.engines > 0 && !held.refusing && !held.skipped)
            .map_or(CHECK, |held| {
                if held.grant_writer.as_deref() == Some(writer) && held.subscription >= 0 {
                    held.subscription
                } else {
                    NEW
                }
            })
    }

    /// Whether the shard may answer a search under a grant from `writer`.
    pub fn granted(&self, uuid: &str, shard: u32, writer: &str, now: i64) -> bool {
        self.holds(uuid, shard, writer)
            && self
                .lock()
                .get(&key(uuid, shard))
                .is_some_and(|held| held.grant_until.saturating_sub(now) > 0)
    }

    /// Whether what the shard holds from `writer`'s grant covers every write
    /// the writer had acknowledged when it granted it, however long ago that
    /// was: the grant's time is not looked at.
    pub fn holds(&self, uuid: &str, shard: u32, writer: &str) -> bool {
        self.lock().get(&key(uuid, shard)).is_some_and(|held| {
            held.engines > 0
                && !held.refusing
                && !held.skipped
                && held.complete
                && held.grant_writer.as_deref() == Some(writer)
        })
    }

    /// Takes a push; the answer names the shards not wanted.
    pub fn handle(&self, request: &PushRequest, now: i64) -> PushResponse {
        let mut unwanted = Vec::new();
        for (shard, records) in &request.shards {
            let number = u32::try_from(shard.shard).unwrap_or(u32::MAX);
            if self.take(&shard.index_uuid, number, records, now).is_none() {
                // ALLOC: at most one per shard of the request, bounded by its frame
                unwanted.push(shard.clone());
            }
        }
        PushResponse { unwanted }
    }
}
