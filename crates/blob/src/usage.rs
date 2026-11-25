//! What each index's work has cost the object store on this node: requests
//! by kind and bytes, kept per index so a tenant's share of the bill can be
//! told apart from its neighbours'.
//!
//! Ported from the shell's `StoreUsage` and `IndexUsage`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/store/StoreUsage.java`).
//! A request is charged to the first that applies:
//!
//! 1. the index its key names ([`Classifier`]);
//! 2. the indices the calling thread is working for ([`Ledger::split`]): an
//!    upload that carries many indices, such as a node-log blob, is shared
//!    by weight, usually their bytes in it;
//! 3. the node itself ([`NODE`]): leases, membership, cluster settings.
//!
//! Every request lands on exactly one line, so the lines add up to what the
//! store counted. Requests are kept in thousandths so a shared request does
//! not round a small index's share to nothing. The ledger tracks at most
//! `max_indices` indices; the rest are counted together as [`OTHER`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::meter::MeterTap;
use crate::op::S3Op;

/// The line for requests no index made.
pub const NODE: &str = "_node";

/// The line for indices past the limit.
pub const OTHER: &str = "_other";

/// One request, in the thousandths requests are kept in.
pub const ONE_REQUEST: u64 = 1_000;

/// Which index a key belongs to, if any; a slice of the key.
pub trait Classifier: Send + Sync {
    /// The index `key` names, or `None` for a key no index owns.
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str>;
}

/// What one index's work has cost since the node started. Counters are only
/// added to, never reset, so a reader takes deltas between two of its reads.
#[derive(Debug, Default)]
pub struct IndexUsage {
    requests: [AtomicU64; S3Op::ALL.len()],
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
}

impl IndexUsage {
    /// Thousandths of a request of `op`.
    pub fn requests(&self, op: S3Op) -> u64 {
        self.requests[op.index()].load(Ordering::Relaxed)
    }

    /// Body bytes read for this index.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Body bytes written for this index.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// The S3 price of everything counted, in thousandths of a PUT: a PUT,
    /// a COPY and a LIST cost one, a GET and a HEAD 0.08 of one
    /// ($0.0004 against $0.005 per thousand), a DELETE nothing.
    pub fn cost_milli_units(&self) -> u64 {
        let mut puts = 0u64;
        let mut gets = 0u64;
        for op in S3Op::ALL {
            match op.price_class() {
                crate::PriceClass::PutClass => puts = puts.saturating_add(self.requests(op)),
                crate::PriceClass::GetClass => gets = gets.saturating_add(self.requests(op)),
                crate::PriceClass::Free => {}
            }
        }
        puts.saturating_add(gets.saturating_mul(8) / 100)
    }

    fn add_requests(&self, op: S3Op, milli: u64) {
        self.requests[op.index()].fetch_add(milli, Ordering::Relaxed);
    }
}

/// Who a thread's unowned requests are charged to: indices by weight.
type Shares = Vec<(Arc<IndexUsage>, u64)>;

thread_local! {
    /// The line the call in progress on this thread belongs to, by its key.
    static OWNER: RefCell<Option<Arc<IndexUsage>>> = const { RefCell::new(None) };
    /// The indices this thread is working for.
    static SHARES: RefCell<Option<Shares>> = const { RefCell::new(None) };
}

/// Restores the owner a call replaced when it ends.
pub(crate) struct OwnerGuard(Option<Arc<IndexUsage>>);

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        OWNER.with(|owner| *owner.borrow_mut() = previous);
    }
}

/// Per-index usage on this node.
#[derive(Debug)]
pub struct Ledger {
    by_index: RwLock<HashMap<String, Arc<IndexUsage>>>,
    node: Arc<IndexUsage>,
    other: Arc<IndexUsage>,
    max_indices: usize,
}

impl Ledger {
    /// An empty ledger tracking at most `max_indices` indices.
    pub fn new(max_indices: usize) -> Self {
        Self {
            by_index: RwLock::new(HashMap::new()), // ALLOC: one empty map, at start.
            node: Arc::default(),
            other: Arc::default(),
            max_indices,
        }
    }

    /// An index's counters, created when there is room; [`OTHER`]'s past the
    /// limit. [`NODE`] and `OTHER` name their own lines.
    pub fn of(&self, index: &str) -> Arc<IndexUsage> {
        if index == NODE {
            return Arc::clone(&self.node);
        }
        if index == OTHER {
            return Arc::clone(&self.other);
        }
        if let Some(found) = self
            .by_index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
        {
            return Arc::clone(found);
        }
        let mut map = self
            .by_index
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = map.get(index) {
            return Arc::clone(found);
        }
        if map.len() >= self.max_indices {
            return Arc::clone(&self.other);
        }
        let made = Arc::<IndexUsage>::default();
        // ALLOC: one name and one counter set an index, at most max_indices.
        map.insert(index.to_owned(), Arc::clone(&made));
        made
    }

    /// Every line, sorted by name: each tracked index, then [`NODE`] and
    /// [`OTHER`].
    pub fn lines(&self) -> Vec<(String, Arc<IndexUsage>)> {
        // ALLOC: one entry an index, once a read of the usage.
        let mut lines: Vec<(String, Arc<IndexUsage>)> = self
            .by_index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(name, usage)| (name.clone(), Arc::clone(usage)))
            .collect(); // ALLOC: one entry an index, once a read.
        lines.sort_by(|a, b| a.0.cmp(&b.0));
        lines.push((NODE.to_owned(), Arc::clone(&self.node))); // ALLOC: one name, once a read.
        lines.push((OTHER.to_owned(), Arc::clone(&self.other))); // ALLOC: one name, once a read.
        lines
    }

    /// The thousandths of a request of `op` summed over every line.
    pub fn total_requests(&self, op: S3Op) -> u64 {
        self.lines()
            .iter()
            .fold(0u64, |sum, (_, u)| sum.saturating_add(u.requests(op)))
    }

    /// Body bytes read and written, summed over every line.
    pub fn total_bytes(&self) -> (u64, u64) {
        self.lines().iter().fold((0u64, 0u64), |(r, w), (_, u)| {
            (
                r.saturating_add(u.bytes_read()),
                w.saturating_add(u.bytes_written()),
            )
        })
    }

    /// Runs `work`, charging the requests it makes on this thread that no
    /// key owns to `weights`' indices in proportion to their weights; a
    /// share is rounded down and the heaviest takes the remainder, so the
    /// shares add up exactly. No weight above zero charges the node.
    pub fn split<T>(&self, weights: &[(&str, u64)], work: impl FnOnce() -> T) -> T {
        // ALLOC: one entry an index of the upload, for the length of the call.
        let shares: Shares = weights
            .iter()
            .filter(|(_, w)| *w > 0)
            .map(|(name, w)| (self.of(name), *w))
            .collect(); // ALLOC: one entry an index of the upload.
        let previous = SHARES.with(|s| s.borrow_mut().replace(shares));
        let done = work();
        SHARES.with(|s| *s.borrow_mut() = previous);
        done
    }

    /// Makes `usage` the line the calls on this thread are for, until the
    /// guard drops.
    pub(crate) fn enter(&self, owner: Option<&str>) -> OwnerGuard {
        let line = owner.map(|index| self.of(index));
        OwnerGuard(OWNER.with(|o| o.replace(line)))
    }

    fn charge(&self, amount: u64, apply: &dyn Fn(&IndexUsage, u64)) {
        if let Some(owner) = OWNER.with(|o| o.borrow().clone()) {
            apply(&owner, amount);
            return;
        }
        let shares = SHARES.with(|s| s.borrow().clone());
        let Some(shares) = shares.filter(|s| !s.is_empty()) else {
            apply(&self.node, amount);
            return;
        };
        let total: u64 = shares.iter().fold(0u64, |t, (_, w)| t.saturating_add(*w));
        let heaviest = shares
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, w))| *w)
            .map_or(0, |(i, _)| i);
        let mut given = 0u64;
        for (i, (usage, weight)) in shares.iter().enumerate() {
            if i == heaviest {
                continue;
            }
            let share = u128::from(amount)
                .checked_mul(u128::from(*weight))
                .and_then(|n| n.checked_div(u128::from(total)))
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0);
            apply(usage, share);
            given = given.saturating_add(share);
        }
        if let Some((usage, _)) = shares.get(heaviest) {
            apply(usage, amount.saturating_sub(given));
        }
    }
}

/// The ledger as the store's meter tap.
#[derive(Debug)]
pub(crate) struct LedgerTap(pub(crate) Arc<Ledger>);

impl MeterTap for LedgerTap {
    fn request(&self, op: S3Op) {
        crate::cost::add_request(op);
        self.0
            .charge(ONE_REQUEST, &|usage, n| usage.add_requests(op, n));
    }

    fn bytes_read(&self, n: u64) {
        crate::cost::add_bytes_read(n);
        self.0.charge(n, &|usage, n| {
            usage.bytes_read.fetch_add(n, Ordering::Relaxed);
        });
    }

    fn bytes_written(&self, n: u64) {
        crate::cost::add_bytes_written(n);
        self.0.charge(n, &|usage, n| {
            usage.bytes_written.fetch_add(n, Ordering::Relaxed);
        });
    }
}
