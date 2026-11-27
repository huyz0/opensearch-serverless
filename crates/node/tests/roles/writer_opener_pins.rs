//! docs/specs/extension-analyzer AC-10, AC-14: a writer opens the shard of an
//! index that pins analyzer modules with them, and refuses to open it
//! without.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use oss_node::writer::{ModuleLoader, PinnedModules};
use oss_writer::bulk::ItemResult;

use super::writer_opener::{create_mapped, failed, rig_with, write_doc};

/// A loader that cannot run a pin.
struct CannotRun;

impl ModuleLoader for CannotRun {
    fn load(&self, _: &[u8], _: &str) -> Result<Option<Arc<dyn PinnedModules>>, String> {
        Err("extension_unavailable: module [m] is not in the store".into())
    }
}

/// A loader whose node has no room to compile the module until `room` is set: the refusal a
/// full extensions pool gives, which carries the words of a capacity refusal.
struct NoRoomYet {
    room: Arc<AtomicBool>,
    then: Arc<Pinned>,
}

impl ModuleLoader for NoRoomYet {
    fn load(&self, _: &[u8], _: &str) -> Result<Option<Arc<dyn PinnedModules>>, String> {
        if self.room.load(Ordering::SeqCst) {
            return Ok(Some(Arc::clone(&self.then) as Arc<dyn PinnedModules>));
        }
        Err(format!(
            "extension_unavailable: module [m]: {} (the pool is full)",
            oss_node::search_modules::NO_CAPACITY
        ))
    }
}

/// A module that keeps a text whole and counts what it is asked.
struct Counted(Arc<AtomicUsize>);

impl oss_ext::analyzer::Analyze for Counted {
    fn analyze(&self, text: &str) -> Result<Vec<oss_ext::analyzer::Token>, oss_ext::CallError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![oss_ext::analyzer::Token {
            term: text.to_owned(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

struct Pinned {
    calls: Arc<AtomicUsize>,
    replays: Arc<AtomicUsize>,
}

impl PinnedModules for Pinned {
    fn analyzers(&self) -> Vec<(String, Arc<dyn oss_ext::analyzer::Analyze>)> {
        vec![(
            "ext_whole".into(),
            Arc::new(Counted(Arc::clone(&self.calls))),
        )]
    }

    fn replaying(&self, _: bool) {
        self.replays.fetch_add(1, Ordering::SeqCst);
    }
}

struct Loads(Arc<Pinned>);

impl ModuleLoader for Loads {
    fn load(&self, _: &[u8], _: &str) -> Result<Option<Arc<dyn PinnedModules>>, String> {
        Ok(Some(Arc::clone(&self.0) as Arc<dyn PinnedModules>))
    }
}

const PINNED_MAPPING: &str = r#"{"properties":{"t":{"type":"text","analyzer":"ext_whole"}}}"#;

// docs/specs/extension-analyzer AC-14: the pin check and the load come before any fencing write
#[test]
fn a_node_that_cannot_run_a_pin_refuses_the_open_and_never_claims_the_head() {
    let r = rig_with(true, Some(Arc::new(CannotRun)));
    create_mapped(&r, "logs", "u1", b"{}", PINNED_MAPPING);
    let (_, reason) = failed(&write_doc(&r.role, "logs", "1", r#"{"t":"a b"}"#));
    assert!(reason.contains("extension_unavailable"), "{reason}");
    assert!(
        r.heads.read("logs", 0).expect("read").is_none(),
        "a node that cannot serve the shard did not fence it"
    );
    assert!(!r.role.held("u1", 0));
}

// docs/specs/extension-analyzer AC-10, AC-14
#[test]
fn a_shard_of_a_pinned_index_indexes_the_field_with_its_module_not_the_built_in_one() {
    let pinned = Arc::new(Pinned {
        calls: Arc::new(AtomicUsize::new(0)),
        replays: Arc::new(AtomicUsize::new(0)),
    });
    let r = rig_with(true, Some(Arc::new(Loads(Arc::clone(&pinned)))));
    create_mapped(&r, "logs", "u1", b"{}", PINNED_MAPPING);
    let first = write_doc(&r.role, "logs", "1", r#"{"t":"Hello World"}"#);
    assert!(matches!(first, ItemResult::Written { .. }), "{first:?}");
    assert_eq!(
        pinned.calls.load(Ordering::SeqCst),
        1,
        "the module was asked"
    );
    assert_eq!(
        pinned.replays.load(Ordering::SeqCst),
        0,
        "a first open replays nothing"
    );
}

// docs/specs/extension-analyzer AC-14: a full pool is a refusal to try again, not a failure of the index
#[test]
fn a_write_that_finds_no_room_to_compile_the_module_is_a_retryable_503_and_the_next_one_opens() {
    let room = Arc::new(AtomicBool::new(false));
    let pinned = Arc::new(Pinned {
        calls: Arc::new(AtomicUsize::new(0)),
        replays: Arc::new(AtomicUsize::new(0)),
    });
    let r = rig_with(
        true,
        Some(Arc::new(NoRoomYet {
            room: Arc::clone(&room),
            then: pinned,
        })),
    );
    create_mapped(&r, "logs", "u1", b"{}", PINNED_MAPPING);
    let (status, reason) = failed(&write_doc(&r.role, "logs", "1", r#"{"t":"a b"}"#));
    assert_eq!(status, 503, "{reason}");
    assert!(reason.contains("no capacity to compile"), "{reason}");
    assert!(
        r.heads.read("logs", 0).expect("read").is_none(),
        "a node with no room did not fence the shard"
    );
    room.store(true, Ordering::SeqCst);
    let again = write_doc(&r.role, "logs", "1", r#"{"t":"a b"}"#);
    assert!(matches!(again, ItemResult::Written { .. }), "{again:?}");
    assert!(r.role.held("u1", 0));
}

/// A loader whose refusal is `why`.
struct Refuses(&'static str);

impl ModuleLoader for Refuses {
    fn load(&self, _: &[u8], _: &str) -> Result<Option<Arc<dyn PinnedModules>>, String> {
        Err(self.0.into())
    }
}

// docs/specs/extension-analyzer AC-12: a failed replay is a typed 503, and so is an unavailable module
#[test]
fn a_refused_open_of_a_pinned_index_is_a_503_typed_by_why() {
    for (why, kind) in [
        (
            "extension_replay_failed: module [m]: out of fuel: applying a record: seq_no 41: x",
            "extension_replay_failed",
        ),
        (
            "extension_unavailable: module [m] is not in the store",
            "extension_unavailable",
        ),
    ] {
        let r = rig_with(true, Some(Arc::new(Refuses(why))));
        create_mapped(&r, "logs", "u1", b"{}", PINNED_MAPPING);
        let ItemResult::Failed { status, error } =
            write_doc(&r.role, "logs", "1", r#"{"t":"a b"}"#)
        else {
            panic!("refused")
        };
        assert_eq!((status, error.kind()), (503, kind), "{why}");
        assert!(error.reason().contains(why), "the words are kept");
    }
}
