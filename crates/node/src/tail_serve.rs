//! What a writer answers a reader's tail fetch: the records it kept past the
//! reader's commit, a subscription so the reader is pushed what follows, and
//! the grant that subscription carries.
//!
//! Ported from `answerFetch`, `claimedThrough`, `gapAfter` and the
//! subscription table of the shell's `TailPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`).
//! Memory only; the transport handler, the push to subscribers and waiting
//! for them are separate pieces.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_cluster::lease::LeaseClock;
use oss_transport::actions::tail::{CHECK, FetchRequest, FetchResponse, NONE, Record};

use crate::tail_keep::TailKeep;

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

struct Subscription {
    id: i64,
    /// When the grant runs out, in milliseconds on the clock.
    grant_until: i64,
    /// The shard's position through which the node has every record.
    confirmed: u64,
    /// The number the next push of this shard to the node takes.
    next_send: u64,
    /// The lowest push number not yet answered.
    next_to_confirm: u64,
    /// Pushes sent and not yet counted, by number: the position each reaches.
    sent: BTreeMap<u64, u64>,
    /// Pushes answered out of order, waiting for an earlier one.
    answered: BTreeSet<u64>,
}

impl Subscription {
    fn new(id: i64, grant_until: i64, confirmed: u64) -> Self {
        Self {
            id,
            grant_until,
            confirmed,
            next_send: 0,
            next_to_confirm: 0,
            sent: BTreeMap::new(),
            answered: BTreeSet::new(),
        }
    }

    fn answered_push(&mut self, number: u64) {
        // A push already counted, or never sent, is not in `sent`.
        if !self.sent.contains_key(&number) {
            return;
        }
        self.answered.insert(number);
        while self.answered.remove(&self.next_to_confirm) {
            if let Some(reached) = self.sent.remove(&self.next_to_confirm) {
                self.confirmed = self.confirmed.max(reached);
            }
            self.next_to_confirm = self.next_to_confirm.saturating_add(1);
        }
    }
}

/// A push's place in the subscription it was sent under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sent {
    /// The subscription's id.
    pub id: i64,
    /// The push's number within it.
    pub number: u64,
}

#[derive(Default)]
struct Serving {
    by_node: HashMap<String, Subscription>,
    next_id: i64,
}

#[derive(Default)]
struct State {
    serving: HashMap<(String, u32), Serving>,
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
        if let Some(serving) = self.lock().serving.get_mut(&key(uuid, shard)) {
            serving.by_node.remove(node);
        }
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
        if let Some(held) = self
            .lock()
            .serving
            .get_mut(&key(uuid, shard))
            .and_then(|serving| serving.by_node.get_mut(node))
            && held.id == sent.id
        {
            held.answered_push(sent.number);
        }
    }

    /// Stops pushing the shard to a node, if it still holds the subscription
    /// the push was sent under: a node that fetched again has a new one,
    /// which what became of an older push says nothing about.
    pub fn drop_sent(&self, uuid: &str, shard: u32, node: &str, sent: Sent) {
        if let Some(serving) = self.lock().serving.get_mut(&key(uuid, shard))
            && serving
                .by_node
                .get(node)
                .is_some_and(|held| held.id == sent.id)
        {
            serving.by_node.remove(node);
        }
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

    /// Stops pushing a shard to every node it is pushed to.
    pub fn unsubscribe_all(&self, uuid: &str, shard: u32) {
        if let Some(serving) = self.lock().serving.get_mut(&key(uuid, shard)) {
            serving.by_node.clear();
        }
    }

    /// Stops keeping and pushing a shard; its claim stays.
    pub fn stop(&self, uuid: &str, shard: u32) {
        self.lock().serving.remove(&key(uuid, shard));
        self.keep.released(uuid, shard);
    }

    /// Forgets a shard this node no longer writes: its subscribers, its claim
    /// and what is kept for it.
    pub fn released(&self, uuid: &str, shard: u32) {
        let mut state = self.lock();
        state.serving.remove(&key(uuid, shard));
        state.claimed.remove(&key(uuid, shard));
        self.keep.released(uuid, shard);
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
