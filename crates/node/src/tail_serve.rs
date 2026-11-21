//! What a writer answers a reader's tail fetch: the records it kept past the
//! reader's commit, a subscription so the reader is pushed what follows, and
//! the grant that subscription carries.
//!
//! Ported from `answerFetch`, `claimedThrough`, `gapAfter` and the
//! subscription table of the shell's `TailPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`).
//! Memory only; the transport handler, the push to subscribers and waiting
//! for them are separate pieces.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_transport::actions::tail::{CHECK, FetchRequest, FetchResponse, NONE, Record};

use crate::tail_keep::TailKeep;
pub use crate::tail_subscription::Sent;
use crate::tail_subscription::{Serving, Subscription};

/// How long a subscription's grant lasts, as the writer counts it: the
/// shell's `GRANT_MILLIS` (500) and `GRANT_MARGIN_MILLIS` (100), so the
/// writer's end is always the later one.
pub const GRANT_MILLIS: i64 = 600;

/// What the node knows of a shard that a fetch's answer depends on.
pub trait ShardFacts: Send + Sync {
    /// Whether this node writes the shard.
    fn writes(&self, uuid: &str, shard: u32) -> bool;
    /// Whether this node's ownership is in doubt: its lease is past its
    /// deadline, or a lapse or revocation has it re-reading its heads.
    fn ownership_in_doubt(&self) -> bool;
    /// Whether the tail overlay is on for the shard.
    fn overlay_on(&self, uuid: &str, shard: u32) -> bool;
    /// The highest sequence number the shard holds, or -1.
    fn max_seq_no(&self, uuid: &str, shard: u32) -> i64;
}

/// A refreshing write waiting for the nodes a shard is pushed to.
struct Waiter {
    key: (String, u32),
    /// The position every node must be confirmed through.
    target: u64,
    pending: HashSet<String>,
    /// A node the write waited for may still be serving without it.
    lost: bool,
    /// Set when no node is waited for any more: whether all were reached.
    done: Option<bool>,
}

/// A wait opened with [`TailServe::open_wait`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitId(u64);

#[derive(Default)]
struct State {
    serving: HashMap<(String, u32), Serving>,
    waiters: BTreeMap<u64, Waiter>,
    next_waiter: u64,
    /// Nodes no longer pushed to that may still answer under a grant, and
    /// when it runs out.
    lapsing: HashMap<(String, u32), HashMap<String, i64>>,
    /// When the node began keeping each shard: a grant an earlier writer
    /// handed out before then may still be running.
    created: HashMap<(String, u32), i64>,
    /// How far each shard's refreshing writes may have been answered as
    /// refreshed.
    claimed: HashMap<(String, u32), i64>,
}

/// Answers tail fetches and remembers whom each shard is pushed to.
pub struct TailServe {
    keep: Arc<TailKeep>,
    facts: Arc<dyn ShardFacts>,
    clock: Arc<dyn LeaseClock>,
    state: Mutex<State>,
    /// Signalled when a waiter may have settled.
    settled: Condvar,
}

fn key(uuid: &str, shard: u32) -> (String, u32) {
    (uuid.to_owned(), shard)
}

pub(crate) fn owned(record: &oss_format::WalRecord<'_>) -> Record {
    Record {
        id: record.id().to_owned(),
        deletion: record.is_deletion(),
        source: record.source().to_owned(),
        seq_no: record.seq_no(),
        primary_term: record.primary_term(),
        version: record.version(),
        routing: record.routing().map(str::to_owned),
    }
}

impl TailServe {
    /// A server over `keep`.
    pub fn new(
        keep: Arc<TailKeep>,
        facts: Arc<dyn ShardFacts>,
        clock: Arc<dyn LeaseClock>,
    ) -> Self {
        Self {
            keep,
            facts,
            clock,
            state: Mutex::new(State::default()),
            settled: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The shard's claim, `highest` the first time. `highest` is read before
    /// the lock is taken, so the facts are never called under it.
    fn claimed(state: &mut State, uuid: &str, shard: u32, highest: i64) -> i64 {
        *state.claimed.entry(key(uuid, shard)).or_insert(highest)
    }

    /// How far a shard's refreshing writes may have been answered as
    /// refreshed: all it holds, the first time it is asked.
    pub fn claimed_through(&self, uuid: &str, shard: u32) -> i64 {
        let highest = self.facts.max_seq_no(uuid, shard);
        Self::claimed(&mut self.lock(), uuid, shard, highest)
    }

    /// Notes that a refreshing write of the shard is about to be answered as
    /// refreshed through what the shard holds.
    pub fn claim(&self, uuid: &str, shard: u32) {
        let highest = self.facts.max_seq_no(uuid, shard);
        let mut state = self.lock();
        let claimed = state.claimed.entry(key(uuid, shard)).or_insert(highest);
        *claimed = (*claimed).max(highest);
    }

    /// The nodes the shard is pushed to.
    pub fn subscribers_of(&self, uuid: &str, shard: u32) -> Vec<String> {
        self.lock()
            .serving
            .get(&key(uuid, shard))
            .map(|serving| serving.by_node.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// When a node's grant on the shard runs out, if it holds one.
    pub fn grant_until(&self, uuid: &str, shard: u32, node: &str) -> Option<i64> {
        self.lock()
            .serving
            .get(&key(uuid, shard))?
            .by_node
            .get(node)
            .map(|subscription| subscription.grant_until)
    }

    /// Stops pushing the shard to a node.
    pub fn unsubscribe(&self, uuid: &str, shard: u32, node: &str) {
        self.drop_node(uuid, shard, node, None, false, false);
    }

    /// Stops pushing the shard to a node, and settles the writes waiting for
    /// it. `sent` names the subscription a push was sent under: a node that
    /// fetched again has a new one, which what became of an older push says
    /// nothing about, so it is left. `may_still_serve` says the node may
    /// answer searches from what it holds without the writes waited for (a
    /// lost push, not an answer saying it has no overlay); `lapses` that it
    /// may go on answering under its grant, which refreshing writes then
    /// wait out.
    pub fn drop_node(
        &self,
        uuid: &str,
        shard: u32,
        node: &str,
        sent: Option<Sent>,
        may_still_serve: bool,
        lapses: bool,
    ) {
        let k = key(uuid, shard);
        let mut state = self.lock();
        let Some(serving) = state.serving.get_mut(&k) else {
            return;
        };
        if let Some(sent) = sent
            && serving
                .by_node
                .get(node)
                .is_none_or(|held| held.id != sent.id)
        {
            return;
        }
        let Some(dropped) = serving.by_node.remove(node) else {
            return;
        };
        if lapses {
            let until = state.lapsing.entry(k.clone()).or_default();
            let at = until.entry(node.to_owned()).or_insert(dropped.grant_until);
            *at = (*at).max(dropped.grant_until);
        }
        for waiter in state.waiters.values_mut().filter(|w| w.key == k) {
            if waiter.pending.remove(node) && may_still_serve {
                waiter.lost = true;
            }
        }
        Self::settle(&mut state);
        self.settled.notify_all();
    }

    /// Settles each waiter that no longer waits for any node.
    fn settle(state: &mut State) {
        let State {
            serving, waiters, ..
        } = state;
        for waiter in waiters.values_mut().filter(|w| w.done.is_none()) {
            let held = serving.get(&waiter.key);
            waiter.pending.retain(|node| {
                !held
                    .and_then(|s| s.by_node.get(node))
                    .is_some_and(|s| s.confirmed >= waiter.target)
            });
            if waiter.pending.is_empty() {
                waiter.done = Some(!waiter.lost);
            }
        }
    }

    /// Ends every wait of a shard as not reached.
    fn fail_waiters(&self, state: &mut State, k: &(String, u32)) {
        for waiter in state.waiters.values_mut().filter(|w| &w.key == k) {
            waiter.done = Some(false);
        }
        self.settled.notify_all();
    }

    /// Numbers a push reaching position `end` for each node the shard is
    /// pushed to, nodes in name order.
    pub fn targets(&self, uuid: &str, shard: u32, end: u64) -> Vec<(String, Sent)> {
        let mut state = self.lock();
        let Some(serving) = state.serving.get_mut(&key(uuid, shard)) else {
            return Vec::new();
        };
        let mut out: Vec<(String, Sent)> = serving
            .by_node
            .iter_mut()
            .map(|(node, subscription)| {
                let number = subscription.next_send;
                subscription.next_send = number.saturating_add(1);
                subscription.sent.insert(number, end);
                (
                    node.clone(),
                    Sent {
                        id: subscription.id,
                        number,
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Counts a push the node answered, if the node still holds the
    /// subscription it was sent under.
    pub fn confirm(&self, uuid: &str, shard: u32, node: &str, sent: Sent) {
        let mut state = self.lock();
        if let Some(held) = state
            .serving
            .get_mut(&key(uuid, shard))
            .and_then(|serving| serving.by_node.get_mut(node))
            && held.id == sent.id
        {
            held.answered_push(sent.number);
            Self::settle(&mut state);
            self.settled.notify_all();
        }
    }

    /// Stops pushing the shard to a node, if it still holds the subscription
    /// the push was sent under: a node that fetched again has a new one,
    /// which what became of an older push says nothing about.
    pub fn drop_sent(&self, uuid: &str, shard: u32, node: &str, sent: Sent) {
        self.drop_node(uuid, shard, node, Some(sent), false, false);
    }

    /// The position through which a node is confirmed to have the shard's
    /// records, if it is subscribed.
    pub fn confirmed(&self, uuid: &str, shard: u32, node: &str) -> Option<u64> {
        self.lock()
            .serving
            .get(&key(uuid, shard))?
            .by_node
            .get(node)
            .map(|held| held.confirmed)
    }

    /// Stops pushing a shard to every node it is pushed to; the writes
    /// waiting for them end as not reached.
    pub fn unsubscribe_all(&self, uuid: &str, shard: u32) {
        let k = key(uuid, shard);
        let mut state = self.lock();
        if let Some(serving) = state.serving.get_mut(&k) {
            serving.by_node.clear();
        }
        self.fail_waiters(&mut state, &k);
    }

    /// Stops keeping and pushing a shard; its claim stays.
    pub fn stop(&self, uuid: &str, shard: u32) {
        let k = key(uuid, shard);
        let mut state = self.lock();
        state.serving.remove(&k);
        state.created.remove(&k);
        self.fail_waiters(&mut state, &k);
        drop(state);
        self.keep.released(uuid, shard);
    }

    /// Forgets a shard this node no longer writes: its subscribers, its claim
    /// and what is kept for it.
    pub fn released(&self, uuid: &str, shard: u32) {
        let k = key(uuid, shard);
        let mut state = self.lock();
        state.serving.remove(&k);
        state.claimed.remove(&k);
        state.created.remove(&k);
        state.lapsing.remove(&k);
        self.fail_waiters(&mut state, &k);
        drop(state);
        self.keep.released(uuid, shard);
    }

    /// Notes that this node began keeping a shard now.
    pub fn note_kept(&self, uuid: &str, shard: u32) {
        let now = self.clock.now_millis();
        self.lock().created.entry(key(uuid, shard)).or_insert(now);
    }

    /// Opens a wait for the nodes the shard is pushed to to be confirmed
    /// through `target`; `None` when none is behind.
    pub fn open_wait(&self, uuid: &str, shard: u32, target: u64) -> Option<WaitId> {
        let k = key(uuid, shard);
        let mut state = self.lock();
        let pending: HashSet<String> = state
            .serving
            .get(&k)?
            .by_node
            .iter()
            .filter(|(_, held)| held.confirmed < target)
            .map(|(node, _)| node.clone())
            .collect();
        if pending.is_empty() {
            return None;
        }
        let id = state.next_waiter;
        state.next_waiter = id.saturating_add(1);
        state.waiters.insert(
            id,
            Waiter {
                key: k,
                target,
                pending,
                lost: false,
                done: None,
            },
        );
        Some(WaitId(id))
    }

    /// Waits at most `timeout` for a wait to settle, and ends it: whether
    /// every node was reached.
    pub fn wait(&self, id: WaitId, timeout: Duration) -> bool {
        let mut state = self.lock();
        let deadline = std::time::Instant::now().checked_add(timeout);
        let reached = loop {
            let Some(waiter) = state.waiters.get(&id.0) else {
                break false;
            };
            if let Some(done) = waiter.done {
                break done;
            }
            let left = deadline.map_or(timeout, |d| {
                d.saturating_duration_since(std::time::Instant::now())
            });
            if left.is_zero() {
                break false;
            }
            state = self
                .settled
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        state.waiters.remove(&id.0);
        reached
    }

    /// The moment, on the clock, by which every grant this node can no longer
    /// reach has run out: those of nodes it stopped pushing to, and any an
    /// earlier writer handed out before this node began keeping the shard.
    pub fn lapse_deadline(&self, uuid: &str, shard: u32) -> i64 {
        let k = key(uuid, shard);
        let now = self.clock.now_millis();
        let mut state = self.lock();
        let began = *state.created.entry(k.clone()).or_insert(now);
        let mut at = began.saturating_add(GRANT_MILLIS);
        if let Some(lapsing) = state.lapsing.get_mut(&k) {
            lapsing.retain(|_, until| *until > now);
            at = lapsing.values().copied().fold(at, i64::max);
        }
        at
    }

    /// Waits on the clock until `at`.
    pub fn pause_until(&self, at: i64) {
        loop {
            let left = at.saturating_sub(self.clock.now_millis());
            let Ok(millis) = u64::try_from(left) else {
                return;
            };
            if millis == 0 {
                return;
            }
            self.clock.pause(Duration::from_millis(millis));
        }
    }

    /// The answer to a fetch.
    pub fn answer(&self, request: &FetchRequest) -> FetchResponse {
        let uuid = request.shard.index_uuid.as_str();
        let shard = u32::try_from(request.shard.shard).unwrap_or(u32::MAX);
        let none = |claimed_through| FetchResponse {
            records: Vec::new(),
            subscription: NONE,
            complete: false,
            claimed_through,
        };
        if !self.facts.writes(uuid, shard) || self.facts.ownership_in_doubt() {
            // Not this node's to say: no grant, and no claim a commit could
            // be compared with.
            return none(i64::MAX);
        }
        let highest = self.facts.max_seq_no(uuid, shard);
        let on = self.facts.overlay_on(uuid, shard);
        let mut state = self.lock();
        if !on
            || request.subscription == CHECK
            || self.keep.gap_after(uuid, shard, request.after_seq_no)
        {
            return none(Self::claimed(&mut state, uuid, shard, highest));
        }
        if !self.keep.keeps(uuid, shard) {
            self.keep.begin(uuid, shard, highest);
            let now = self.clock.now_millis();
            state.created.entry(key(uuid, shard)).or_insert(now);
            let claimed = state.claimed.entry(key(uuid, shard)).or_insert(highest);
            *claimed = (*claimed).max(highest);
        }
        let grant_until = self.clock.now_millis().saturating_add(GRANT_MILLIS);
        // Read before the records are: a record acknowledged between the two
        // is both in the answer and pushed, and `confirmed` is never too
        // high.
        let position = self.keep.position(uuid, shard);
        let serving = state.serving.entry(key(uuid, shard)).or_default();
        if let Some(held) = serving.by_node.get_mut(&request.subscriber)
            && request.subscription >= 0
            && held.id == request.subscription
        {
            held.grant_until = held.grant_until.max(grant_until);
            let id = held.id;
            return FetchResponse {
                records: Vec::new(),
                subscription: id,
                complete: true,
                claimed_through: Self::claimed(&mut state, uuid, shard, highest),
            };
        }
        let id = serving.next_id;
        serving.next_id = serving.next_id.saturating_add(1);
        serving.by_node.insert(
            request.subscriber.clone(),
            Subscription::new(id, grant_until, position),
        );
        let claimed_through = Self::claimed(&mut state, uuid, shard, highest);
        drop(state);
        // Subscribed before the records are read, so a write acknowledged in
        // between is pushed if it is not in the answer.
        let (kept, complete) = self.keep.snapshot_after(uuid, shard, request.after_seq_no);
        FetchResponse {
            records: kept.iter().map(owned).collect(),
            subscription: id,
            complete,
            claimed_through,
        }
    }
}
