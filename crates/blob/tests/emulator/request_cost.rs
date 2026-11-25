//! A request's own store requests (docs/specs/request-cost-scope/SPEC.md).

use std::sync::Arc;

use crate::{MIB, get_all, pattern, put};
use oss_blob::cost::{self, RequestCost};
use oss_blob::usage::{Classifier, Ledger, ONE_REQUEST};
use oss_blob::{ByteRange, Emulator, S3Op, UsageStore};

struct Prefixed;

impl Classifier for Prefixed {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}

fn store() -> (UsageStore<Emulator>, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::new(8));
    let store = UsageStore::new(
        Emulator::new(64 * MIB, 1),
        Arc::clone(&ledger),
        Arc::new(Prefixed),
    );
    (store, ledger)
}

// AC-1
#[test]
fn a_scope_counts_its_requests_by_kind_and_leaves_the_ledger_as_it_was() {
    let (s, ledger) = store();
    let cost = RequestCost::new();
    {
        let _scope = cost::enter(Some(Arc::clone(&cost)));
        put(&s, "i/a/one", &pattern(100));
        get_all(&s, "i/a/one", ByteRange::Full).unwrap();
        get_all(&s, "i/a/one", ByteRange::Full).unwrap();
    }
    assert_eq!(cost.requests(S3Op::Put), 1);
    assert_eq!(cost.requests(S3Op::Get), 2);
    assert_eq!(cost.bytes_written(), 100);
    assert_eq!(cost.bytes_read(), 200);
    assert_eq!(ledger.of("a").requests(S3Op::Put), ONE_REQUEST);
    assert_eq!(ledger.of("a").requests(S3Op::Get), 2 * ONE_REQUEST);
}

// AC-2
#[test]
fn a_thread_with_no_scope_counts_nowhere() {
    let (s, _) = store();
    let cost = RequestCost::new();
    put(&s, "i/a/one", b"x");
    assert_eq!(cost.requests(S3Op::Put), 0);
    assert!(cost::capture().is_none());
}

// AC-3
#[test]
fn a_scope_entered_on_a_spawned_thread_counts_into_the_same_counters() {
    let (s, _) = store();
    let cost = RequestCost::new();
    let _scope = cost::enter(Some(Arc::clone(&cost)));
    let handle = cost::capture().expect("a scope is entered");
    std::thread::scope(|threads| {
        threads.spawn(|| {
            assert!(cost::capture().is_none(), "a new thread has none");
            let _there = cost::enter(Some(handle));
            put(&s, "i/a/there", b"x");
        });
    });
    put(&s, "i/a/here", b"x");
    assert_eq!(cost.requests(S3Op::Put), 2);
}

// AC-4
#[test]
fn a_guard_restores_the_scope_it_replaced_even_on_a_panic() {
    let (s, _) = store();
    let outer = RequestCost::new();
    let inner = RequestCost::new();
    {
        let _a = cost::enter(Some(Arc::clone(&outer)));
        {
            let _b = cost::enter(Some(Arc::clone(&inner)));
            put(&s, "i/a/inner", b"x");
        }
        put(&s, "i/a/outer", b"x");
    }
    assert_eq!(
        (inner.requests(S3Op::Put), outer.requests(S3Op::Put)),
        (1, 1)
    );
    assert!(cost::capture().is_none());
    let lost = RequestCost::new();
    let thread = std::thread::spawn({
        let lost = Arc::clone(&lost);
        move || {
            let _scope = cost::enter(Some(lost));
            panic!("a handler failed");
        }
    });
    assert!(thread.join().is_err());
    // The same worker would serve the next request: a new thread stands in,
    // and the guard's drop on unwind is shown by a scoped panic caught here.
    let caught = std::panic::catch_unwind(|| {
        let _scope = cost::enter(Some(RequestCost::new()));
        panic!("a handler failed");
    });
    assert!(caught.is_err());
    assert!(cost::capture().is_none(), "the unwind cleared the scope");
    put(&s, "i/a/after", b"x");
    assert_eq!(lost.requests(S3Op::Put), 0);
}

// AC-5
#[test]
fn a_suspended_scope_counts_nothing_until_it_is_back() {
    let (s, _) = store();
    let cost = RequestCost::new();
    let _scope = cost::enter(Some(Arc::clone(&cost)));
    {
        let _quiet = cost::suspend();
        put(&s, "i/a/shared", b"x");
    }
    put(&s, "i/a/own", b"x");
    assert_eq!(cost.requests(S3Op::Put), 1);
}

// AC-6
#[test]
fn the_price_is_the_ledgers() {
    let (s, _) = store();
    let cost = RequestCost::new();
    {
        let _scope = cost::enter(Some(Arc::clone(&cost)));
        put(&s, "i/a/one", b"x");
        get_all(&s, "i/a/one", ByteRange::Full).unwrap();
    }
    assert_eq!(cost.cost_milli_units(), 1_000 + 80);
}
