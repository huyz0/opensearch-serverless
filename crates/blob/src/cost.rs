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

thread_local! {
    /// The request this thread is doing store work for, if it asked.
    static SCOPE: RefCell<Option<Arc<RequestCost>>> = const { RefCell::new(None) };
}

/// The scope of this thread, to be [`enter`]ed on a thread it hands store
/// work to.
#[must_use]
pub fn capture() -> Option<Arc<RequestCost>> {
    SCOPE.with(|s| s.borrow().clone())
}

/// Restores the scope a call replaced when it ends, also on unwind.
#[must_use = "the scope ends when the guard drops"]
pub struct ScopeGuard(Option<Arc<RequestCost>>);

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|s| *s.borrow_mut() = previous);
    }
}

/// Makes `cost` the scope of this thread until the guard drops.
pub fn enter(cost: Option<Arc<RequestCost>>) -> ScopeGuard {
    ScopeGuard(SCOPE.with(|s| s.replace(cost)))
}

/// No scope on this thread until the guard drops: work that serves many
/// requests, such as a commit window's append, is no one request's.
pub fn suspend() -> ScopeGuard {
    enter(None)
}

/// Adds one request of `op` to this thread's scope.
pub(crate) fn add_request(op: S3Op) {
    with_scope(|c| {
        c.requests[op.index()].fetch_add(ONE_REQUEST, Ordering::Relaxed);
    });
}

/// Adds `n` body bytes read to this thread's scope.
pub(crate) fn add_bytes_read(n: u64) {
    with_scope(|c| {
        c.bytes_read.fetch_add(n, Ordering::Relaxed);
    });
}

/// Adds `n` body bytes written to this thread's scope.
pub(crate) fn add_bytes_written(n: u64) {
    with_scope(|c| {
        c.bytes_written.fetch_add(n, Ordering::Relaxed);
    });
}

fn with_scope(add: impl FnOnce(&RequestCost)) {
    SCOPE.with(|s| {
        if let Some(cost) = s.borrow().as_deref() {
            add(cost);
        }
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
}
