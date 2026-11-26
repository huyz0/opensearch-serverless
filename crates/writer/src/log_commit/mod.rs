//! Logged commits: a staged commit rides the node log, under its shard's
//! commit key, so a successor can open from it instead of the older register
//! (docs/specs/logged-commits).
//!
//! The record joins the window now open ([`NodeLogCommitter::enqueue`]), so a
//! node that is logging makes no write for it; [`LoggedCommits::tick`] writes
//! what nothing carried after two windows. Only the newest record of a shard
//! is held: a landed one releases the one before it to the log's trim, and a
//! register write releases the one it covers.

mod record;

pub use record::{CommitRecord, KEY_SUFFIX, commit_key, is_commit_key, shard_uuid};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_format::{ShardKey, WalRecord};

use crate::log::{CommitError, NodeLogCommitter, Position};

/// What is held of one shard's commits.
#[derive(Default)]
struct Held {
    /// The newest landed record: where, and its generation.
    newest: Option<(Position, u64)>,
    /// The generation the register covers.
    covered: Option<u64>,
    /// Counts the times the shard was let go: a record staged before one is
    /// released where it lands, not held.
    era: u64,
}

#[derive(Default)]
struct State {
    held: BTreeMap<ShardKey, Held>,
    /// When the oldest record nothing has carried was enqueued.
    waiting_since: Option<Instant>,
}

/// The commits a node has logged, newest of each shard.
pub struct LoggedCommits {
    committer: Arc<NodeLogCommitter>,
    window: Duration,
    state: Arc<Mutex<State>>,
}

impl LoggedCommits {
    /// Logging through `committer`, whose windows are `window` long.
    pub fn new(committer: Arc<NodeLogCommitter>, window: Duration) -> Self {
        Self {
            committer,
            window,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Logs `commit`, the `generation`th staged commit of `shard`, in the
    /// window now open; it is held once it lands.
    ///
    /// # Errors
    ///
    /// [`CommitError`] if the record cannot be encoded.
    pub fn staged(
        &self,
        shard: &ShardKey,
        term: i64,
        commit: &CommitRecord,
        generation: u64,
    ) -> Result<(), CommitError> {
        let key = commit_key(shard);
        let text = commit.encode();
        let record = WalRecord::index_unsequenced(record::RECORD_ID, text.as_str());
        let (committer, state) = (Arc::clone(&self.committer), Arc::clone(&self.state));
        let landed_key = key.clone();
        let era = lock(&self.state).held.entry(key.clone()).or_default().era;
        self.committer.enqueue(&key, term, &[record], move |at| {
            landed(&committer, &state, &landed_key, at, generation, era);
        })?;
        lock(&self.state)
            .waiting_since
            .get_or_insert_with(Instant::now);
        Ok(())
    }

    /// Writes what no other record carried once two windows have passed:
    /// one write for every shard waiting, none before then or when nothing waits.
    pub fn tick(&self) {
        {
            let mut state = lock(&self.state);
            let due = state
                .waiting_since
                .is_some_and(|t| t.elapsed() >= self.window.saturating_mul(2));
            if !due {
                return;
            }
            state.waiting_since = None;
        }
        self.committer.flush_queued();
    }

    /// The register now covers `shard`'s commits up to `generation`: the held
    /// record is released if that is the newest.
    pub fn checkpointed(&self, shard: &ShardKey, generation: u64) {
        let key = commit_key(shard);
        let mut state = lock(&self.state);
        let held = state.held.entry(key.clone()).or_default();
        held.covered = Some(held.covered.map_or(generation, |c| c.max(generation)));
        if let Some((at, _)) = held.newest.filter(|(_, g)| *g <= generation) {
            held.newest = None;
            drop(state);
            self.committer.on_published(&key, at);
        }
    }

    /// Lets go of a shard this node released: nothing of it is held.
    pub fn forget(&self, shard: &ShardKey) {
        let key = commit_key(shard);
        let newest = {
            let mut state = lock(&self.state);
            let held = state.held.entry(key.clone()).or_default();
            // The entry stays, one small record a shard seen, so a record
            // still in a window is known stale when it lands.
            held.era = held.era.saturating_add(1);
            held.covered = None;
            held.newest.take()
        };
        if let Some((at, _)) = newest {
            self.committer.on_published(&key, at);
        }
        self.committer.forget(&key);
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A record of `generation` landed at `at`: it is kept if it is the newest
/// the register does not cover, and released otherwise, with whatever it replaces.
fn landed(
    committer: &NodeLogCommitter,
    state: &Mutex<State>,
    key: &ShardKey,
    at: Position,
    generation: u64,
    era: u64,
) {
    let release = {
        let mut state = lock(state);
        let held = state.held.entry(key.clone()).or_default();
        let covered = held.era != era || held.covered.is_some_and(|c| generation <= c);
        let older = held.newest.is_some_and(|(_, g)| generation < g);
        if covered || older {
            at
        } else {
            let before = held.newest.replace((at, generation));
            match before {
                Some((p, _)) => p,
                None => return,
            }
        }
    };
    committer.on_published(key, release);
}
