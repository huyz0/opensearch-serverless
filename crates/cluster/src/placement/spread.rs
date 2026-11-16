//! Spreading a hot shard's searches over more than one reader. Ported from
//! `cluster/HotShardSpread.java`: a shard's search rate (the higher of a
//! 10 s and a 1 s decayed average) sets how many of its ranked readers are
//! eligible, one per `target_per_second` up to `max_readers`, and of two
//! eligible readers picked at random the one with fewer searches running is
//! asked first (two choices, which keeps the busiest reader off the top).
//! What it counts is this node's own traffic: nothing is shared or stored.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::hash::{ShardKey, decimal};
use crate::lease::lock;

/// The search rate one reader is meant to carry, a second (the Java's
/// `serverless.search.reader_spread.target_per_second` default).
pub const DEFAULT_TARGET_PER_SECOND: f64 = 50.0;
/// The most readers one shard's searches spread over
/// (`serverless.search.reader_spread.max_readers`).
pub const DEFAULT_MAX_READERS: usize = 4;
/// The slow average's decay.
const DECAY_NANOS: f64 = 10_000_000_000.0;
/// The fast average's decay, which catches a burst.
const BURST_NANOS: f64 = 1_000_000_000.0;
/// The most shards whose rate is kept; past it the cold ones are dropped.
pub const MAX_TRACKED: usize = 50_000;

/// How spread a shard's searches are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadConfig {
    /// The rate one reader carries; positive.
    pub target_per_second: f64,
    /// The most readers a shard spreads over; at least 1.
    pub max_readers: usize,
}

impl Default for SpreadConfig {
    fn default() -> Self {
        Self {
            target_per_second: DEFAULT_TARGET_PER_SECOND,
            max_readers: DEFAULT_MAX_READERS,
        }
    }
}

/// A monotonic clock in nanoseconds; the tests drive their own.
pub type Nanos = Arc<dyn Fn() -> i64 + Send + Sync>;

/// One node's view of how hot each shard it searches is, and how many
/// searches it has running on each reader.
pub struct HotShardSpread {
    config: SpreadConfig,
    nanos: Nanos,
    rates: Mutex<HashMap<String, Rate>>,
    in_flight: Mutex<HashMap<String, Arc<AtomicUsize>>>,
    picks: AtomicU64,
    random: RandomState,
}

/// One search running on a reader, counted until dropped.
pub struct Running(Arc<AtomicUsize>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl HotShardSpread {
    /// A spread with `config`, timed by `nanos`; `None` for a target that
    /// is not positive or no readers.
    pub fn new(config: SpreadConfig, nanos: Nanos) -> Option<Self> {
        if config.target_per_second.is_nan()
            || config.target_per_second <= 0.0
            || config.max_readers < 1
        {
            return None;
        }
        Some(Self {
            config,
            nanos,
            // ALLOC: one entry a searched shard, at most MAX_TRACKED.
            rates: Mutex::new(HashMap::new()),
            // ALLOC: one entry a reader this node searches, at most the fleet.
            in_flight: Mutex::new(HashMap::new()),
            picks: AtomicU64::new(0),
            random: RandomState::new(),
        })
    }

    /// The most readers a shard spreads over.
    pub fn max_readers(&self) -> usize {
        self.config.max_readers
    }

    /// Counts one search of `key` and orders its `ranked` readers for it
    /// (`order`).
    pub fn order<'a>(&self, key: ShardKey<'_>, ranked: &[&'a str]) -> Vec<&'a str> {
        let rate = self.record(key);
        self.pick(self.width(rate), ranked)
    }

    /// Orders `ranked` for `key` by the rate already counted, counting
    /// nothing (`orderCounted`).
    pub fn order_counted<'a>(&self, key: ShardKey<'_>, ranked: &[&'a str]) -> Vec<&'a str> {
        self.pick(self.width_of(key), ranked)
    }

    /// `key`'s search rate a second, 0 for a shard not searched.
    pub fn per_second(&self, key: ShardKey<'_>) -> f64 {
        let now = (self.nanos)();
        let mut digits = [0u8; 11];
        lock(&self.rates)
            .get_mut(&name(key, &mut digits))
            .map_or(0.0, |rate| rate.per_second(now))
    }

    /// How many readers `key`'s searches spread over now.
    pub fn width_of(&self, key: ShardKey<'_>) -> usize {
        let rate = self.per_second(key);
        if rate == 0.0 { 1 } else { self.width(rate) }
    }

    /// Searches this node has running on `node_id`.
    pub fn in_flight(&self, node_id: &str) -> usize {
        lock(&self.in_flight)
            .get(node_id)
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    /// Counts a search on `node_id` until the guard is dropped.
    pub fn begin(&self, node_id: &str) -> Running {
        let count = Arc::clone(
            lock(&self.in_flight)
                // ALLOC: one entry a reader this node searches, at most the fleet.
                .entry(node_id.to_owned())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0))),
        );
        count.fetch_add(1, Ordering::Relaxed);
        Running(count)
    }

    fn width(&self, per_second: f64) -> usize {
        let wanted = (per_second / self.config.target_per_second).ceil();
        let max = self.config.max_readers;
        // A width past the bound is the bound; the cast only sees a value
        // already below it.
        if wanted >= max as f64 {
            max
        } else if wanted >= 1.0 {
            wanted as usize
        } else {
            1
        }
    }

    fn pick<'a>(&self, spread: usize, ranked: &[&'a str]) -> Vec<&'a str> {
        let width = spread.min(ranked.len());
        // ALLOC: as many as the ranked readers given, a handful.
        let mut ordered = Vec::with_capacity(ranked.len());
        if width <= 1 {
            ordered.extend_from_slice(ranked);
            return ordered;
        }
        let a = self.below(width);
        let mut b = self.below(width.saturating_sub(1));
        if b >= a {
            b = b.saturating_add(1);
        }
        let chosen = if self.in_flight(ranked[b]) < self.in_flight(ranked[a]) {
            b
        } else {
            a
        };
        ordered.push(ranked[chosen]);
        ordered.extend(
            ranked
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != chosen)
                .map(|(_, r)| *r),
        );
        ordered
    }

    /// A random number below `n`, which is at least 1.
    fn below(&self, n: usize) -> usize {
        let draw = self
            .random
            .hash_one(self.picks.fetch_add(1, Ordering::Relaxed));
        let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
        usize::try_from(draw.checked_rem(n).unwrap_or(0)).unwrap_or(0)
    }

    fn record(&self, key: ShardKey<'_>) -> f64 {
        let now = (self.nanos)();
        let mut digits = [0u8; 11];
        let name = name(key, &mut digits);
        let mut rates = lock(&self.rates);
        if rates.len() >= MAX_TRACKED && !rates.contains_key(&name) {
            let cold = self.config.target_per_second / 10.0;
            rates.retain(|_, rate| rate.per_second(now) >= cold);
        }
        rates.entry(name).or_default().add(now)
    }
}

/// `index#shard`, the key the Java counts a shard's rate under.
fn name(key: ShardKey<'_>, digits: &mut [u8; 11]) -> String {
    let shard = decimal(key.shard, digits);
    // ALLOC: one shard name a search, the index name and a few digits.
    let mut name = String::with_capacity(key.index.len().saturating_add(12));
    name.push_str(key.index);
    name.push('#');
    name.push_str(shard);
    name
}

/// Two exponentially decayed counts of searches, a second.
#[derive(Debug, Default)]
struct Rate {
    slow: f64,
    fast: f64,
    at: Option<i64>,
}

impl Rate {
    fn add(&mut self, now: i64) -> f64 {
        self.decay(now);
        self.slow += 1e9 / DECAY_NANOS;
        self.fast += 1e9 / BURST_NANOS;
        self.slow.max(self.fast)
    }

    fn per_second(&mut self, now: i64) -> f64 {
        self.decay(now);
        self.slow.max(self.fast)
    }

    fn decay(&mut self, now: i64) {
        if let Some(at) = self.at
            && now > at
        {
            let elapsed = now.saturating_sub(at) as f64;
            self.slow *= (-elapsed / DECAY_NANOS).exp();
            self.fast *= (-elapsed / BURST_NANOS).exp();
        }
        self.at = Some(self.at.map_or(now, |at| at.max(now)));
    }
}
