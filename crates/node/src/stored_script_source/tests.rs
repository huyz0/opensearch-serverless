//! The cache's cost and freshness, over a metered emulator and a clock the
//! test moves.

use std::sync::atomic::AtomicU64;

use oss_blob::{Emulator, S3Op};
use oss_mem::BudgetConfig;

use super::*;

const TTL: u64 = 10_000;

struct Rig {
    store: Arc<Emulator>,
    now: Arc<AtomicU64>,
    source: Arc<ScriptSource>,
}

fn rig_with(request_bodies: usize) -> Rig {
    let store = Arc::new(Emulator::new(1 << 30, 3));
    let now = Arc::new(AtomicU64::new(1_000));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::RequestBodies, request_bodies)
            .expect("fits"),
    );
    let clock = Arc::clone(&now);
    let source = ScriptSource::open(
        &(Arc::clone(&store) as Arc<dyn BlobStore>),
        "",
        &budget,
        TTL,
        Box::new(move || clock.load(Ordering::SeqCst)),
    )
    .expect("opens");
    Rig { store, now, source }
}

fn rig() -> Rig {
    rig_with(64 << 20)
}

fn requests(r: &Rig) -> u64 {
    S3Op::ALL.iter().map(|op| r.store.meter().count(*op)).sum()
}

fn put(r: &Rig, id: &str, body: &str) {
    DocSet::new(
        Arc::clone(&r.store) as Arc<dyn BlobStore>,
        "",
        DocKind::StoredScript,
    )
    .expect("set")
    .put(id, body.as_bytes(), false, &mut vec![0u8; 2 << 20])
    .expect("put");
}

fn painless(source: &str) -> String {
    format!(r#"{{"script":{{"lang":"painless","source":"{source}"}}}}"#)
}

// docs/specs/stored-script-ids AC-5
#[test]
fn a_node_that_resolves_nothing_makes_no_request_and_a_ttl_makes_one_marker_get() {
    let r = rig();
    assert_eq!(requests(&r), 0, "opening and idling cost nothing");
    put(&r, "a", &painless("1"));
    let before = requests(&r);
    assert_eq!(r.source.resolve("a").expect("found").source, "1");
    let first = requests(&r) - before;
    assert!(first > 0, "the first resolve reads the set");
    let warm = requests(&r);
    for _ in 0..50 {
        r.source.resolve("a").expect("found");
    }
    assert_eq!(requests(&r), warm, "within the TTL a resolve is memory");
    r.now.fetch_add(TTL, Ordering::SeqCst);
    r.source.resolve("a").expect("found");
    assert_eq!(requests(&r) - warm, 1, "past the TTL one marker GET");
}

// docs/specs/stored-script-ids AC-5
#[test]
fn a_changed_script_is_seen_after_the_ttl_and_a_missing_one_is_missing() {
    let r = rig();
    assert_eq!(r.source.resolve("a"), Err(ResolveError::Missing));
    put(&r, "a", &painless("1"));
    assert_eq!(
        r.source.resolve("a"),
        Err(ResolveError::Missing),
        "another node's put is not seen within the TTL"
    );
    r.now.fetch_add(TTL, Ordering::SeqCst);
    assert_eq!(r.source.resolve("a").expect("found").source, "1");
    put(&r, "a", &painless("2"));
    assert_eq!(r.source.resolve("a").expect("old").source, "1");
    r.now.fetch_add(TTL, Ordering::SeqCst);
    assert_eq!(r.source.resolve("a").expect("new").source, "2");
}

// docs/specs/stored-script-ids AC-8
#[test]
fn a_put_here_is_seen_at_the_next_resolve_even_after_a_check_that_was_in_flight() {
    let r = rig();
    put(&r, "a", &painless("1"));
    r.source.resolve("a").expect("found");
    put(&r, "a", &painless("2"));
    r.source.invalidate();
    assert_eq!(r.source.resolve("a").expect("new").source, "2");
    // A change that lands while a refresh reads: the generation the refresh
    // began at is stale, so its check does not count.
    let before_check = r.source.generation.load(Ordering::SeqCst);
    r.source.invalidate();
    {
        let mut state = r.source.state.lock().unwrap();
        state.checked = Some((r.now.load(Ordering::SeqCst), before_check));
    }
    let requests_before = requests(&r);
    put(&r, "a", &painless("3"));
    assert_eq!(r.source.resolve("a").expect("new").source, "3");
    assert!(
        requests(&r) > requests_before,
        "the stale check was not trusted"
    );
}

// docs/specs/stored-script-ids AC-6
#[test]
fn a_set_past_the_bound_and_an_unreadable_script_are_refused() {
    let r = rig();
    put(&r, "junk", r#"{"x":1,"y":2}"#);
    assert!(matches!(
        r.source.resolve("junk"),
        Err(ResolveError::Failed(_))
    ));
    let big = painless(&"a".repeat((1 << 20) - 200));
    for i in 0..9 {
        put(&r, &format!("big{i}"), &big);
    }
    r.now.fetch_add(TTL, Ordering::SeqCst);
    assert!(matches!(
        r.source.resolve("big0"),
        Err(ResolveError::TooBig(_))
    ));
    let tight = rig_with(MAX_DOC_BYTES);
    put(&tight, "a", &painless("1"));
    assert!(matches!(
        tight.source.resolve("a"),
        Err(ResolveError::TooBig(_))
    ));
}

// docs/specs/stored-script-ids AC-7
#[test]
fn resolves_that_arrive_together_make_one_refresh() {
    let r = rig();
    put(&r, "a", &painless("1"));
    let one = {
        let probe = rig();
        put(&probe, "a", &painless("1"));
        let before = requests(&probe);
        probe.source.resolve("a").expect("found");
        requests(&probe) - before
    };
    let before = requests(&r);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                assert_eq!(r.source.resolve("a").expect("found").source, "1");
            });
        }
    });
    assert_eq!(requests(&r) - before, one, "one refresh served all eight");
}
