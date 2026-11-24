//! Hibernating a writer that has gone idle: its engine is closed and its
//! memory freed while the shard's head stays this node's, so the next write
//! wakes it at the same term instead of taking it over.
//!
//! Ported from `ShardHibernation` and `WriterHibernation` in huyz0/OpenSearch
//! `feature/serverlessshell`. A writer sleeps only once everything it
//! acknowledged is in a published commit, under the shard's lock, so the
//! closed engine held nothing the commit lacks; the head still names this
//! process at the term it slept at, and nobody can take it while this node's
//! lease is live. If somebody did, the head read on waking says so and the
//! shard is opened the ordinary way. Held in memory only: a restarted node
//! has a new ephemeral id and takes what it left the ordinary way.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use oss_types::settings::{LongSetting, Scope, Setting};

use super::{Key, ShardSlot, WriterRole, lock};

/// `serverless.activation.hibernate_after_millis`: how long a shard may go
/// unused before its engine is closed. Default 0, never: waking costs requests
/// a shard that stays open does not, so it is off until that is measured to
/// cost nothing it saves.
pub const HIBERNATE_AFTER_MILLIS: LongSetting = LongSetting::new(
    "serverless.activation.hibernate_after_millis",
    0,
    0,
    604_800_000,
    Scope::Node,
    "How long a writer shard may go unused before its engine is closed and its memory freed, in \
     milliseconds; its head stays this node's and the next write wakes it. 0 never hibernates.",
);

/// `serverless.activation.give_back_after_millis`: how long a hibernated
/// shard may sleep before its head is given back. Default 0, never: the next
/// write then pays a take that a wake does not, so it is off until that is
/// measured to cost nothing it saves.
pub const GIVE_BACK_AFTER_MILLIS: LongSetting = LongSetting::new(
    "serverless.activation.give_back_after_millis",
    0,
    0,
    86_400_000,
    Scope::Node,
    "How long a hibernated writer shard may sleep before its head is given back, in \
     milliseconds; the next write takes it the ordinary way. 0 never gives back.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[
    HIBERNATE_AFTER_MILLIS.setting(),
    GIVE_BACK_AFTER_MILLIS.setting(),
];

/// A shard whose engine is closed: the term its head carried when it slept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Slept {
    pub(super) term: i64,
    /// When it slept, in the pass's clock: a record older than
    /// [`KEEP_ASLEEP_MILLIS`] is dropped.
    at: u64,
}

/// How long a hibernated shard's record is kept: after a day unused it is
/// forgotten, so the shard of an index deleted meanwhile does not stay in
/// memory for ever, and a write that does come takes the shard the ordinary
/// way. Memory only, no request.
pub const KEEP_ASLEEP_MILLIS: u64 = 86_400_000;

/// The most heads one pass gives back, so a bulk load that falls idle all at
/// once is given back over several passes and does not hold the timer for
/// every shard's two requests; the rest stay asleep, and are due next pass.
pub const GIVE_BACK_PER_PASS: usize = 64;

/// The hibernated shards of a role, and how long one must be idle.
#[derive(Default)]
pub(super) struct Hibernation {
    after_millis: u64,
    give_back_after_millis: u64,
    asleep: Mutex<HashMap<Key, Slept>>,
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HibernatePass {
    /// Shards put to sleep.
    pub hibernated: usize,
    /// Hibernated shards whose head was given back.
    pub given_back: usize,
    /// Idle shards left open because a write or a read reached them while
    /// the pass was publishing: a later pass tries them again.
    pub busy: usize,
    /// Idle shards whose publish failed, by `[uuid][shard]` and the reason.
    pub failed: Vec<String>,
}

impl WriterRole {
    /// Hibernates shards unused for `millis`, when [`Self::hibernate_due`]
    /// is called; 0 never.
    #[must_use]
    pub fn with_hibernation(mut self, millis: u64) -> Self {
        self.hibernation.after_millis = millis;
        self
    }

    /// Gives back the head of a shard asleep for `millis`, when
    /// [`Self::give_back_due`] is called; 0 never.
    #[must_use]
    pub fn with_give_back(mut self, millis: u64) -> Self {
        self.hibernation.give_back_after_millis = millis;
        self
    }

    /// The pass the timer runs: [`Self::hibernate_idle`] at the role's
    /// threshold, then [`Self::give_back_due`]; nothing at 0.
    pub fn hibernate_due(&self, now_millis: u64) -> HibernatePass {
        let mut pass = match self.hibernation.after_millis {
            0 => return HibernatePass::default(),
            idle => self.hibernate_idle(idle, now_millis),
        };
        pass.given_back = self.give_back_due(now_millis);
        pass
    }

    /// [`Self::give_back_idle`] at the role's threshold, nothing at 0.
    pub fn give_back_due(&self, now_millis: u64) -> usize {
        match self.hibernation.give_back_after_millis {
            0 => 0,
            after => self.give_back_idle(after, now_millis),
        }
    }

    /// Gives back the head of each shard asleep for `after_millis` at
    /// `now_millis`: the record is claimed first, as a wake claims it, so one
    /// of the two wins, then the opener releases the head if it still names
    /// this process at the term the shard slept at. Answers how many heads
    /// were released. A shard none of which is due makes no request.
    pub fn give_back_idle(&self, after_millis: u64, now_millis: u64) -> usize {
        let Some(opener) = &self.opener else {
            return 0;
        };
        // ALLOC: one entry a shard due, once a pass.
        let due: Vec<(Key, Slept)> = {
            let mut asleep = lock(&self.hibernation.asleep);
            let keys: Vec<Key> = asleep
                .iter()
                .filter(|(_, s)| now_millis.saturating_sub(s.at) >= after_millis)
                .map(|(k, _)| k.clone())
                .take(GIVE_BACK_PER_PASS)
                .collect();
            keys.into_iter()
                .filter_map(|k| asleep.remove(&k).map(|s| (k, s)))
                .collect()
        };
        let mut given = 0usize;
        for ((uuid, shard), slept) in due {
            let Some(index) = self.names.name_of(&uuid) else {
                continue;
            };
            if opener.give_back(&index, &uuid, shard, slept.term) {
                given = given.saturating_add(1);
            }
        }
        given
    }

    /// Puts to sleep each held shard unused for `idle_millis` at `now_millis`.
    /// A shard with writes no commit holds is published first, and sleeps only
    /// if nothing reached it meanwhile. Idleness is read from memory: a pass
    /// over shards none of which is idle makes no request.
    pub fn hibernate_idle(&self, idle_millis: u64, now_millis: u64) -> HibernatePass {
        let mut pass = HibernatePass::default();
        self.forget_stale(now_millis);
        let Some(publishing) = &self.publishing else {
            return pass;
        };
        // ALLOC: one handle a held shard, once a pass.
        let held: Vec<(Key, Arc<Mutex<ShardSlot>>)> = lock(&self.shards)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        for (key, slot) in held {
            if self.pinned(&key.0) {
                continue;
            }
            let (used, max_seq_no) = {
                let slot = lock(&slot);
                (slot.used_at(), slot.max_seq_no())
            };
            if now_millis.saturating_sub(used) < idle_millis {
                continue;
            }
            let Some(index) = self.names.name_of(&key.0) else {
                continue;
            };
            if publishing.unpublished(&key, max_seq_no) {
                match self.publish_shard(publishing, &key, &index, &slot, false) {
                    Ok(Some((done, max_seq_no))) => {
                        if let Some(t) = self.tail.get() {
                            t.published(&key.0, key.1, max_seq_no);
                        }
                        self.staged(&key.0, key.1, &done.manifest);
                    }
                    Ok(None) => {}
                    Err(reason) => {
                        // ALLOC: error path; one message a failed shard.
                        pass.failed.push(format!("[{}][{}] {reason}", key.0, key.1));
                        continue;
                    }
                }
            }
            let mut held = lock(&slot);
            if held.asleep()
                || held.released()
                || held.used_at() != used
                || publishing.unpublished(&key, held.max_seq_no())
            {
                pass.busy = pass.busy.saturating_add(1);
                continue;
            }
            held.sleep();
            // Recorded before the shard leaves the map, so a write arriving
            // meanwhile finds it asleep and wakes it.
            lock(&self.hibernation.asleep).insert(
                key.clone(),
                Slept {
                    term: held.term(),
                    at: now_millis,
                },
            );
            self.release(&key.0, key.1, &slot);
            pass.hibernated = pass.hibernated.saturating_add(1);
        }
        pass
    }

    /// Drops the records of shards asleep for [`KEEP_ASLEEP_MILLIS`].
    fn forget_stale(&self, now_millis: u64) {
        lock(&self.hibernation.asleep)
            .retain(|_, slept| now_millis.saturating_sub(slept.at) < KEEP_ASLEEP_MILLIS);
    }

    /// Records a shard closed at `term` as asleep, so its next write wakes
    /// it at the same term.
    pub(super) fn remember_asleep(&self, key: &Key, term: i64, now_millis: u64) {
        lock(&self.hibernation.asleep).insert(
            key.clone(),
            Slept {
                term,
                at: now_millis,
            },
        );
    }

    /// Whether the shard is hibernated here.
    pub fn asleep(&self, uuid: &str, shard: u32) -> bool {
        // ALLOC: one key a lookup.
        lock(&self.hibernation.asleep).contains_key(&(uuid.to_owned(), shard))
    }

    /// How many shards are hibernated here.
    pub fn hibernated(&self) -> usize {
        lock(&self.hibernation.asleep).len()
    }

    /// Takes the record of a hibernated shard, so one write wakes it.
    pub(super) fn take_asleep(&self, uuid: &str, shard: u32) -> Option<Slept> {
        // ALLOC: one key a lookup.
        lock(&self.hibernation.asleep).remove(&(uuid.to_owned(), shard))
    }

    /// A woken shard's commit is what its engine holds: it is not due for a
    /// publish until a write reaches it.
    pub(super) fn woke(&self, uuid: &str, shard: u32) {
        let Some(publishing) = &self.publishing else {
            return;
        };
        if let Some(slot) = self.slot(uuid, shard) {
            // ALLOC: one key a wake.
            publishing.published(&(uuid.to_owned(), shard), lock(&slot).max_seq_no());
        }
    }
}
