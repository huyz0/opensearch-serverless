//! docs/specs/extension-analyzer AC-12: a node taking over a shard of an
//! index that pins a module replays the previous writer's unpublished writes
//! through the module, fails the open closed when a replayed call fails, and
//! opens once the cause is gone, with no write lost.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_ext::analyzer::{Analyze, Token};
use oss_ext::{CallError, Status};
use oss_format::IndexRecord;
use oss_node::writer::{ModuleLoader, PinnedModules};
use oss_writer::bulk::ItemResult;

use super::writer_takeover::{Cluster, Node, cluster, node_built, seq, takeover, write_source};

const MAPPING: &str =
    r#"{"properties":{"n":{"type":"integer"},"t":{"type":"text","analyzer":"ext_whole"}}}"#;

/// A module that keeps a text whole, counts its calls and fails them while
/// `fail` is set and a replay is running.
struct Flaky {
    fail: AtomicBool,
    replaying: AtomicBool,
    calls: AtomicUsize,
    replayed: AtomicUsize,
    failure: Mutex<Option<String>>,
}

struct Whole(Arc<Flaky>);

impl Analyze for Whole {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let m = &self.0;
        m.calls.fetch_add(1, Ordering::SeqCst);
        if m.replaying.load(Ordering::SeqCst) {
            m.replayed.fetch_add(1, Ordering::SeqCst);
            if m.fail.load(Ordering::SeqCst) {
                *m.failure.lock().expect("lock") = Some("module [m]: out of fuel".into());
                return Err(CallError::from_status(Status::Limit, b"fuel"));
            }
        }
        Ok(vec![Token {
            term: text.to_owned(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

/// The set of modules a [`Flaky`] module makes.
struct Shared(Arc<Flaky>);

impl PinnedModules for Shared {
    fn analyzers(&self) -> Vec<(String, Arc<dyn Analyze>)> {
        vec![("ext_whole".into(), Arc::new(Whole(Arc::clone(&self.0))))]
    }

    fn replaying(&self, on: bool) {
        self.0.replaying.store(on, Ordering::SeqCst);
    }

    fn replay_failure(&self) -> Option<String> {
        self.0.failure.lock().expect("lock").take()
    }
}

struct Loads(Arc<Flaky>);

impl ModuleLoader for Loads {
    fn load(&self, _: &[u8], _: &str) -> Result<Option<Arc<dyn PinnedModules>>, String> {
        Ok(Some(Arc::new(Shared(Arc::clone(&self.0)))))
    }
}

fn flaky(fail: bool) -> Arc<Flaky> {
    Arc::new(Flaky {
        fail: AtomicBool::new(fail),
        replaying: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        replayed: AtomicUsize::new(0),
        failure: Mutex::new(None),
    })
}

const LONG: Duration = Duration::from_secs(3_600);

/// node-1 writes `a` and publishes it, writes `b` and stages it, then `c` that
/// only its log holds: the writes a successor must replay.
fn lost_predecessor(c: &Cluster) -> Node {
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    c.descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");
    let live = flaky(false);
    let one = node_built(
        c,
        "node-1",
        1,
        false,
        None,
        Some(LONG),
        (true, false, None),
        (None, Some(Arc::new(Loads(live)))),
    );
    let doc = |t: &str| format!(r#"{{"t":"{t}"}}"#);
    assert_eq!(
        seq(&write_source(&one.role, "logs", "a", &doc("alpha one"))),
        0
    );
    assert_eq!(one.role.publish_due().published, 1, "the fence writes");
    assert_eq!(
        seq(&write_source(&one.role, "logs", "b", &doc("beta two"))),
        1
    );
    assert_eq!(one.role.publish_due().published, 1, "b is staged");
    std::thread::sleep(Duration::from_millis(60));
    one.role.publish_due();
    assert_eq!(
        seq(&write_source(&one.role, "logs", "c", &doc("gamma three"))),
        2
    );
    one
}

fn successor(c: &Cluster, one: &Node, module: &Arc<Flaky>) -> Node {
    let calls = Arc::new(AtomicUsize::new(0));
    node_built(
        c,
        "node-2",
        2,
        true,
        Some(takeover(c, one, &calls)),
        None,
        (false, true, None),
        (None, Some(Arc::new(Loads(Arc::clone(module))))),
    )
}

// AC-12: a takeover replays what the predecessor logged through the module
#[test]
fn a_takeover_replays_the_unpublished_writes_through_the_module() {
    let c = cluster();
    let one = lost_predecessor(&c);
    let module = flaky(false);
    let two = successor(&c, &one, &module);
    let first = write_source(&two.role, "logs", "d", r#"{"t":"delta four"}"#);
    assert_eq!(seq(&first), 3, "after a, b and c, none lost: {first:?}");
    assert!(
        module.replayed.load(Ordering::SeqCst) >= 1,
        "the write only the log held (c) went through the module, not the built-in analyzer"
    );
    assert!(
        !module.replaying.load(Ordering::SeqCst),
        "back to live writes"
    );
}

// AC-12: a failed replayed call fails the open closed; the cause gone, it opens with nothing lost
#[test]
fn a_replay_that_fails_a_call_fails_the_open_and_the_next_one_opens_with_nothing_lost() {
    let c = cluster();
    let one = lost_predecessor(&c);
    let module = flaky(true);
    let two = successor(&c, &one, &module);
    let refused = write_source(&two.role, "logs", "d", r#"{"t":"delta four"}"#);
    let ItemResult::Failed { status, error } = &refused else {
        panic!("the open is refused: {refused:?}")
    };
    assert_eq!((*status, error.kind()), (503, "extension_replay_failed"));
    assert!(error.reason().contains("out of fuel"), "{}", error.reason());
    assert!(
        !two.role.held("u1", 0),
        "a shard whose replay failed is not held"
    );
    assert!(
        !module.replaying.load(Ordering::SeqCst),
        "the failed replay leaves live writes on"
    );

    // The factor raised and the shard reopened: the same replay now passes.
    module.fail.store(false, Ordering::SeqCst);
    let again = write_source(&two.role, "logs", "d", r#"{"t":"delta four"}"#);
    assert_eq!(
        seq(&again),
        3,
        "every acknowledged write is still there: {again:?}"
    );
    assert!(two.role.held("u1", 0));
}
