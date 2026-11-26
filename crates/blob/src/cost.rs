//! What one request's store requests cost: counters that live for a request
//! that asked to see them, and the thread scope the meter's tap adds to.
//!
//! A scope is set on the thread serving the request and, through
//! [`capture`] and [`enter`], on each thread that does store work for it. A
//! thread with no scope adds nothing anywhere.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::op::{PriceClass, S3Op};
use crate::usage::ONE_REQUEST;

/// The store requests and body bytes made for one request.
#[derive(Debug, Default)]
pub struct RequestCost {
    requests: [AtomicU64; S3Op::ALL.len()],
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
}

impl RequestCost {
    /// Counters at zero, shared by every thread working for the request.
    #[must_use]
    pub fn new() -> Arc<Self> {
        // ALLOC: one counter set a request that asked, freed with it.
        Arc::new(Self::default())
    }

    /// Thousandths of a request of `op`.
    pub fn milli_requests(&self, op: S3Op) -> u64 {
        self.requests[op.index()].load(Ordering::Relaxed)
    }

    /// Whole requests of `op`, a share rounded to the nearest.
    pub fn requests(&self, op: S3Op) -> u64 {
        self.milli_requests(op)
            .saturating_add(ONE_REQUEST / 2)
            .checked_div(ONE_REQUEST)
            .unwrap_or(0)
    }

    /// Body bytes read.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Body bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// The price of everything counted, in thousandths of a PUT, as the
    /// ledger prices it: PUT, COPY and LIST one, GET and HEAD 0.08, DELETE
    /// nothing.
    pub fn cost_milli_units(&self) -> u64 {
        let mut puts = 0u64;
        let mut gets = 0u64;
        for op in S3Op::ALL {
            match op.price_class() {
                PriceClass::PutClass => puts = puts.saturating_add(self.milli_requests(op)),
                PriceClass::GetClass => gets = gets.saturating_add(self.milli_requests(op)),
                PriceClass::Free => {}
            }
        }
        puts.saturating_add(gets.saturating_mul(8) / 100)
    }
}

/// What a thread's store requests are counted for.
#[derive(Clone, Debug)]
pub enum Scope {
    /// One request's counters.
    One(Arc<RequestCost>),
    /// Work that serves several requests, such as a commit window's append:
    /// each member gets its weight's share of every request and byte.
    Split(Arc<Split>),
}

/// The members of a splitting scope and the total their weights are shares of.
#[derive(Debug)]
pub struct Split {
    members: Vec<(Arc<RequestCost>, u64)>,
    total: u64,
}

impl Scope {
    /// A scope that gives each of `members` its weight over `total`. A
    /// total past the members' weights is the part of the work that no
    /// asking request owns, and is counted nowhere.
    #[must_use]
    pub fn split(members: Vec<(Arc<RequestCost>, u64)>, total: u64) -> Self {
        // ALLOC: one split a commit window that has an asking writer.
        Self::Split(Arc::new(Split {
            members,
            total: total.max(1),
        }))
    }
}

thread_local! {
    /// What this thread is doing store work for, if a request asked.
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// The request scope of this thread, to be [`enter`]ed on a thread it hands
/// store work to. A thread in a splitting scope has none: carry it with
/// [`capture_scope`].
#[must_use]
pub fn capture() -> Option<Arc<RequestCost>> {
    SCOPE.with(|s| match s.borrow().as_ref() {
        Some(Scope::One(cost)) => Some(Arc::clone(cost)),
        _ => None,
    })
}

/// The scope of this thread, of either kind, to be [`enter_scope`]d on a
/// thread it hands store work to.
#[must_use]
pub fn capture_scope() -> Option<Scope> {
    SCOPE.with(|s| s.borrow().clone())
}

/// Restores the scope a call replaced when it ends, also on unwind.
#[must_use = "the scope ends when the guard drops"]
pub struct ScopeGuard(Option<Scope>);

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|s| *s.borrow_mut() = previous);
    }
}

/// Makes `cost` the scope of this thread until the guard drops.
pub fn enter(cost: Option<Arc<RequestCost>>) -> ScopeGuard {
    enter_scope(cost.map(Scope::One))
}

/// Makes `scope` the scope of this thread until the guard drops.
pub fn enter_scope(scope: Option<Scope>) -> ScopeGuard {
    ScopeGuard(SCOPE.with(|s| s.replace(scope)))
}

/// No scope on this thread until the guard drops: work that serves many
/// requests, such as a commit window's append, is no one request's.
pub fn suspend() -> ScopeGuard {
    enter(None)
}

/// `n` of something, scaled by `weight` over `total`.
fn share(n: u64, weight: u64, total: u64) -> u64 {
    u64::try_from(
        u128::from(n)
            .saturating_mul(u128::from(weight))
            .checked_div(u128::from(total))
            .unwrap_or(0),
    )
    .unwrap_or(u64::MAX)
}

/// Adds `n` to this thread's scope through `add`: all of it to one
/// request, or each member's share to its own.
fn for_each(n: u64, add: impl Fn(&RequestCost, u64)) {
    SCOPE.with(|s| match s.borrow().as_ref() {
        Some(Scope::One(cost)) => add(cost, n),
        Some(Scope::Split(split)) => {
            for (cost, weight) in &split.members {
                add(cost, share(n, *weight, split.total));
            }
        }
        None => {}
    });
}

/// Adds one request of `op` to this thread's scope.
pub(crate) fn add_request(op: S3Op) {
    for_each(ONE_REQUEST, |c, n| {
        c.requests[op.index()].fetch_add(n, Ordering::Relaxed);
    });
}

/// Adds `n` body bytes read to this thread's scope.
pub(crate) fn add_bytes_read(n: u64) {
    for_each(n, |c, n| {
        c.bytes_read.fetch_add(n, Ordering::Relaxed);
    });
}

/// Adds `n` body bytes written to this thread's scope.
pub(crate) fn add_bytes_written(n: u64) {
    for_each(n, |c, n| {
        c.bytes_written.fetch_add(n, Ordering::Relaxed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/specs/request-cost-scope AC-6
    #[test]
    fn a_share_rounds_to_the_nearest_whole_request() {
        let cost = RequestCost::default();
        for (milli, whole) in [(0, 0), (499, 0), (500, 1), (1_499, 1), (1_500, 2)] {
            cost.requests[S3Op::Get.index()].store(milli, Ordering::Relaxed);
            assert_eq!(cost.requests(S3Op::Get), whole, "{milli}");
        }
    }

    // docs/specs/request-cost-window AC-1
    #[test]
    fn a_splitting_scope_gives_each_member_its_weights_share() {
        let (a, b) = (RequestCost::new(), RequestCost::new());
        {
            let _g = enter_scope(Some(Scope::split(
                vec![(Arc::clone(&a), 1), (Arc::clone(&b), 3)],
                4,
            )));
            add_request(S3Op::Put);
            add_bytes_written(400);
        }
        assert_eq!(
            (a.milli_requests(S3Op::Put), b.milli_requests(S3Op::Put)),
            (250, 750)
        );
        assert_eq!((a.bytes_written(), b.bytes_written()), (100, 300));
        // Outside a scope nothing is added anywhere.
        add_request(S3Op::Put);
        assert_eq!(a.milli_requests(S3Op::Put), 250);
        // A total past the members' weights leaves the rest unreported.
        let c = RequestCost::new();
        {
            let _g = enter_scope(Some(Scope::split(vec![(Arc::clone(&c), 1)], 3)));
            add_request(S3Op::Put);
        }
        assert_eq!(c.milli_requests(S3Op::Put), 333);
    }

    // docs/specs/request-cost-window AC-2
    #[test]
    fn a_splitting_scope_carries_to_another_thread_and_ends_with_its_guard() {
        let a = RequestCost::new();
        let scope = Scope::split(vec![(Arc::clone(&a), 1), (RequestCost::new(), 1)], 2);
        {
            let _g = enter_scope(Some(scope));
            let carried = capture_scope();
            std::thread::spawn(move || {
                let _g = enter_scope(carried);
                add_request(S3Op::Get);
            })
            .join()
            .unwrap();
            assert_eq!(a.milli_requests(S3Op::Get), 500);
            // The single-request capture does not see a split.
            assert!(capture().is_none());
        }
        // A panic inside a guarded scope leaves the thread with none.
        let caught = std::panic::catch_unwind(|| {
            let _g = enter_scope(Some(Scope::split(vec![(RequestCost::new(), 1)], 1)));
            panic!("inside");
        });
        assert!(caught.is_err());
        add_request(S3Op::Get);
        assert_eq!(a.milli_requests(S3Op::Get), 500);
    }
}
