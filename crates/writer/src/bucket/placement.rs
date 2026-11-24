//! Which writer a bucket nobody holds should go to.
//!
//! Ported from `BucketPlacement` (huyz0/OpenSearch
//! `feature/serverlessshell`). The highest-ranked live writer for the bucket
//! by rendezvous hashing over the writer leases every node already reads:
//! every node computes the same answer with no request and no coordinator,
//! and a node joining or leaving moves about one bucket in N.
//!
//! A node is ranked only once it has been seen for a hold-down, so one that
//! restarts or flaps does not pull buckets back and forth; one that returns
//! under a new incarnation soon after the last doubles its hold-down, up to
//! thirty minutes. A writer holding more than 1.25 times the mean is passed
//! over while another has room. A held bucket stays while its holder ranks in
//! the top two. A disagreement between nodes costs at most a forwarded
//! write: the head's compare-and-swap decides ownership.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use oss_cluster::routing::murmur3_x64_h1;
use oss_format::{NodeLease, java_order};

/// How long a newly seen writer waits before buckets are placed on it.
pub const DEFAULT_HOLD_DOWN_MILLIS: i64 = 120_000;

/// The longest hold-down a flapping writer is given.
pub const MAX_HOLD_DOWN_MILLIS: i64 = 1_800_000;

/// How much above the fleet's mean a writer may hold and still be placed on.
pub const LOAD_BOUND: f64 = 1.25;

/// How many top-ranked writers may keep a bucket they hold.
pub const KEEP_RANKS: usize = 2;

/// When a node was first seen under its current incarnation.
#[derive(Debug, Clone)]
struct Seen {
    ephemeral_id: String,
    since: i64,
    hold_down: i64,
}

/// Rendezvous placement of buckets over the writers, with a hold-down.
#[derive(Debug)]
pub struct BucketPlacement {
    base_hold_down: i64,
    seen: Mutex<HashMap<String, Seen>>,
}

/// A bucket's score on a node; the highest wins.
fn score(bucket_id: &str, node_id: &str) -> i64 {
    // ALLOC: one key of two ids, per node ranked.
    let mut key = Vec::with_capacity(
        bucket_id
            .len()
            .saturating_add(node_id.len())
            .saturating_add(1),
    );
    key.extend_from_slice(bucket_id.as_bytes());
    key.push(0);
    key.extend_from_slice(node_id.as_bytes());
    // The Java compares the hash as a signed long.
    i64::from_ne_bytes(murmur3_x64_h1(&key).to_ne_bytes())
}

impl BucketPlacement {
    /// A placement whose newly seen writers wait `hold_down_millis` (zero or
    /// less for none).
    #[must_use]
    pub fn new(hold_down_millis: i64) -> Self {
        Self {
            base_hold_down: hold_down_millis.max(0),
            // ALLOC: empty until a node is seen.
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Notes the writers in a membership view, starting the hold-down of
    /// each one not seen before.
    pub fn observe(&self, leases: &[NodeLease], now: i64) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        self.observe_in(&mut seen, leases, now);
    }

    fn observe_in(&self, seen: &mut HashMap<String, Seen>, leases: &[NodeLease], now: i64) {
        let first_view = seen.is_empty();
        for lease in leases {
            match seen.get(&lease.node_id) {
                None => {
                    // The first view this placement has: everyone already
                    // there counts as settled, or a fresh node would wait
                    // the hold-down before placing anything at all.
                    let since = if first_view { i64::MIN / 2 } else { now };
                    // ALLOC: one entry per node ever seen, a fleet's size.
                    seen.insert(
                        lease.node_id.clone(),
                        Seen {
                            // ALLOC: one id per entry, as above.
                            ephemeral_id: lease.ephemeral_id.clone(),
                            since,
                            hold_down: self.base_hold_down,
                        },
                    );
                }
                Some(before)
                    if !lease.ephemeral_id.is_empty()
                        && lease.ephemeral_id != before.ephemeral_id =>
                {
                    // A new incarnation. Soon after the last one came up it
                    // is flapping: wait twice as long.
                    let flapping =
                        now.saturating_sub(before.since) < before.hold_down.saturating_mul(2);
                    let hold_down = if flapping {
                        before
                            .hold_down
                            .max(1)
                            .saturating_mul(2)
                            .min(MAX_HOLD_DOWN_MILLIS)
                    } else {
                        self.base_hold_down
                    };
                    // ALLOC: replaces the entry, the same size.
                    let ephemeral_id = lease.ephemeral_id.clone();
                    seen.insert(
                        lease.node_id.clone(),
                        Seen {
                            ephemeral_id,
                            since: now,
                            hold_down,
                        },
                    );
                }
                Some(_) => {}
            }
        }
    }

    /// Ranks the live writers for a bucket, best first. Writers still in
    /// their hold-down are left out while any settled writer is live;
    /// writers past the load bound are put last.
    pub fn rank(
        &self,
        bucket_id: &str,
        leases: &[NodeLease],
        now: i64,
        writer: impl Fn(&NodeLease) -> bool,
    ) -> Vec<String> {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        self.observe_in(&mut seen, leases, now);
        let live: Vec<&NodeLease> = leases
            .iter()
            .filter(|l| !l.is_expired_at(now) && writer(l))
            // ALLOC: references to the leases passed in, a fleet's size.
            .collect();
        let settled: Vec<&NodeLease> = live
            .iter()
            .copied()
            .filter(|l| {
                seen.get(&l.node_id)
                    .is_none_or(|s| now.saturating_sub(s.since) >= s.hold_down)
            })
            // ALLOC: as above.
            .collect();
        drop(seen);
        let mut candidates = if settled.is_empty() { live } else { settled };
        let (total, reporting) = candidates
            .iter()
            .filter(|l| l.held >= 0)
            .fold((0.0_f64, 0.0_f64), |(total, n), l| {
                (total + f64::from(l.held), n + 1.0)
            });
        // Nobody reporting gives a mean of zero, which no load is over.
        let mean = total / reporting.max(1.0);
        let overloaded = |l: &NodeLease| f64::from(l.held) > LOAD_BOUND * mean;
        candidates.sort_by(|a, b| {
            overloaded(a)
                .cmp(&overloaded(b))
                .then_with(|| score(bucket_id, &b.node_id).cmp(&score(bucket_id, &a.node_id)))
                .then_with(|| java_order(&a.node_id, &b.node_id))
        });
        // ALLOC: the ranked ids, a fleet's size.
        candidates.iter().map(|l| l.node_id.clone()).collect()
    }

    /// The writer an unheld bucket should go to, or `None` with no live
    /// writer.
    pub fn owner_for(
        &self,
        bucket_id: &str,
        leases: &[NodeLease],
        now: i64,
        writer: impl Fn(&NodeLease) -> bool,
    ) -> Option<String> {
        self.rank(bucket_id, leases, now, writer).into_iter().next()
    }

    /// Whether a writer holding a bucket should keep it: while it ranks in
    /// the top [`KEEP_RANKS`], so a joining node takes over only the buckets
    /// it is clearly better for.
    pub fn keeps(
        &self,
        bucket_id: &str,
        owner_node_id: &str,
        leases: &[NodeLease],
        now: i64,
        writer: impl Fn(&NodeLease) -> bool,
    ) -> bool {
        self.rank(bucket_id, leases, now, writer)
            .iter()
            .position(|id| id == owner_node_id)
            .is_some_and(|at| at < KEEP_RANKS)
    }
}
