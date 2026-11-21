//! docs/specs/reader-overlay-source: a search served over the overlay an
//! installed source finished, and from the commit alone without one.

#[path = "support/mod.rs"]
mod support;

use std::sync::{Arc, Mutex};

use lucene_store::ByteBuffersDirectory;
use oss_blob::S3Op;
use oss_format::WalRecord;
use oss_mem::MemoryBudget;
use oss_node::overlay_builder::TailFetcher;
use oss_node::tail_overlay::{TAIL_OVERLAY_ENABLED, install, wire};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{CommitAccess, Overlay, OverlaySource, Ready, find_copies};
use oss_testkit::TempDir;
use oss_transport::actions::tail::{FetchRequest, FetchResponse, Record};
use oss_writer::mapping::Mapping;

use support::request;

#[path = "overlay_search_rig/mod.rs"]
mod rig;

use rig::*;

/// Builds the overlay of a replaced id, a deleted one and a new one on the
/// commit it is handed, as often as it is asked unless it has built one.
struct Source {
    mapping: Mapping,
    asked: Mutex<u32>,
    built: Mutex<Option<Arc<Ready>>>,
    ready: bool,
    /// The id of the third record: changing it makes the next overlay differ.
    new_id: Mutex<&'static str>,
    /// The shards an engine closed on, in order.
    closed: Mutex<Vec<(String, u32)>>,
}

impl Source {
    fn new(ready: bool) -> Arc<Self> {
        Arc::new(Self {
            mapping: Mapping::parse(br#"{"properties":{"n":{"type":"integer"}}}"#)
                .expect("mapping"),
            asked: Mutex::new(0),
            built: Mutex::new(None),
            ready,
            new_id: Mutex::new("c"),
            closed: Mutex::new(Vec::new()),
        })
    }
}

impl Source {
    /// The next `ready` builds an overlay whose new document has `id`.
    fn replace_with(&self, id: &'static str) {
        *self.new_id.lock().expect("lock") = id;
        *self.built.lock().expect("lock") = None;
    }

    fn views(&self) -> u64 {
        self.built
            .lock()
            .expect("lock")
            .as_ref()
            .map_or(0, |r| r.views_made())
    }
}

impl OverlaySource for Source {
    fn ready(
        &self,
        uuid: &str,
        shard: u32,
        _stage: &Stage,
        commit: &Arc<dyn CommitAccess>,
        _now: u64,
    ) -> Option<Arc<Ready>> {
        assert_eq!((uuid, shard), ("u1", 0));
        assert_eq!(
            commit.mapping_json(),
            r#"{"properties":{"n":{"type":"integer"}}}"#,
            "the source gets the mapping the descriptor holds"
        );
        assert_eq!(
            commit.index_name(),
            "logs",
            "the source gets the name of the index it serves"
        );
        let mut asked = self.asked.lock().expect("lock");
        *asked = asked.saturating_add(1);
        if !self.ready {
            return None;
        }
        let mut built = self.built.lock().expect("lock");
        if built.is_none() {
            let records = [
                WalRecord::index("a".to_owned(), r#"{"n":7}"#.to_owned(), 1000, 1, 1001),
                WalRecord::deletion("b".to_owned(), 1001, 1, 1002),
                WalRecord::index(
                    self.new_id.lock().expect("lock").to_owned(),
                    r#"{"n":9}"#.to_owned(),
                    1002,
                    1,
                    1003,
                ),
            ];
            let overlay = Overlay::build(&self.mapping, &records, 0, 4 << 20).expect("builds");
            let copies = find_copies(commit.reader(), overlay.ids()).expect("copies");
            *built = Some(Arc::new(Ready::new(overlay, copies)));
        }
        built.clone()
    }

    fn closed(&self, uuid: &str, shard: u32, _stage: &Stage, _now: u64) {
        self.closed
            .lock()
            .expect("lock")
            .push((uuid.to_owned(), shard));
    }
}

fn n_is(fx: &Fixture<'_>, n: i64) -> u64 {
    let reply = request(
        fx.server.local_addr(),
        "POST",
        "/logs/_search",
        Some(
            format!(r#"{{"size":0,"track_total_hits":true,"query":{{"term":{{"n":{n}}}}}}}"#)
                .as_bytes(),
        ),
    );
    assert_eq!(reply.status, 200);
    total(&String::from_utf8_lossy(&reply.body))
}

#[test]
fn with_the_setting_off_no_source_is_installed_and_the_commit_answers() {
    let root = TempDir::new("overlay-off");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(!install(
        &fx.service,
        TAIL_OVERLAY_ENABLED.default(),
        source.clone()
    ));
    assert_eq!(all(&fx), 2);
    assert_eq!(*source.asked.lock().expect("lock"), 0);
}

#[test]
fn an_installed_source_s_overlay_replaces_deletes_and_adds_what_a_search_sees() {
    let root = TempDir::new("overlay-on");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    assert!(install(&fx.service, true, Source::new(true)));
    // a became n=7, b is deleted, c is new: a, c remain.
    assert_eq!(all(&fx), 2);
    assert_eq!(n_is(&fx, 7), 1);
    assert_eq!(n_is(&fx, 9), 1);
    assert_eq!(n_is(&fx, 1), 0);
}

#[test]
fn a_source_with_nothing_finished_leaves_the_commits_answer() {
    let root = TempDir::new("overlay-none");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(false);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    assert_eq!(n_is(&fx, 1), 2);
    assert!(*source.asked.lock().expect("lock") >= 2);
}

#[test]
fn searches_between_two_overlays_share_one_view_and_make_no_request() {
    let root = TempDir::new("overlay-view");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    let before = counts(&fx.store);
    for _ in 0..3 {
        assert_eq!(all(&fx), 2);
    }
    let after = counts(&fx.store);
    for op in [S3Op::Get, S3Op::List, S3Op::Head, S3Op::Put, S3Op::Delete] {
        assert_eq!(gets(before, after, op), 0, "{op:?}");
    }
    assert_eq!(source.views(), 1, "four searches made one view");
}

#[test]
fn a_new_overlay_from_the_source_makes_a_new_view_once() {
    let root = TempDir::new("overlay-swap");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(n_is(&fx, 9), 1);
    source.replace_with("d");
    for _ in 0..3 {
        assert_eq!(n_is(&fx, 9), 1);
    }
    assert_eq!(source.views(), 1, "the new overlay was viewed once");
}

#[test]
fn the_setting_is_off_by_default() {
    assert!(!TAIL_OVERLAY_ENABLED.default());
}

#[test]
fn a_source_is_told_when_the_engine_it_served_closes() {
    let root = TempDir::new("overlay-closed");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let source = Source::new(true);
    assert!(install(&fx.service, true, source.clone()));
    assert_eq!(all(&fx), 2);
    assert!(source.closed.lock().expect("lock").is_empty());
    let Fixture {
        service, server, ..
    } = fx;
    server.shutdown();
    drop(service);
    assert_eq!(
        *source.closed.lock().expect("lock"),
        [("u1".to_owned(), 0)],
        "told once, for the shard it served"
    );
}

/// A writer that has acknowledged one write past every commit it is asked
/// about.
struct Writer;

impl TailFetcher for Writer {
    fn fetch(&self, _writer: &str, request: &FetchRequest) -> Option<FetchResponse> {
        Some(FetchResponse {
            records: vec![Record {
                id: "x".to_owned(),
                deletion: false,
                source: r#"{"n":5}"#.to_owned(),
                seq_no: request.after_seq_no.saturating_add(1),
                primary_term: 1,
                version: 1,
                routing: None,
            }],
            subscription: 1,
            complete: true,
            claimed_through: i64::MAX,
        })
    }
}

fn wired(fx: &Fixture<'_>, enabled: bool) -> Option<oss_node::tail_overlay::Wired> {
    wire(
        &fx.service,
        enabled,
        MemoryBudget::new(&support::budget_config()),
        Arc::new(Writer),
        "reader-1".to_owned(),
        Arc::new(|| 1_000),
    )
}

#[test]
fn a_wired_node_fetches_the_writers_records_into_the_overlay_a_search_sees() {
    let root = TempDir::new("overlay-wired");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    let wired = wired(&fx, true).expect("wired");
    // The first search opens the shard and serves the commit; the builder
    // asks the writer in the background.
    assert_eq!(all(&fx), 2, "the commit serves while nothing is built");
    assert!(wired.builder.settle(), "never settled");
    assert_eq!(wired.held.next_ordinal("u1", 0), 1, "the write is held");
    // The fetch and the build are one job: the overlay is finished.
    assert_eq!(all(&fx), 3);
    assert_eq!(n_is(&fx, 5), 1);
    assert!(wired.held.granted("u1", 0, "node-1", 1_000));
}

#[test]
fn with_the_setting_off_nothing_is_wired_and_a_second_wire_is_refused() {
    let root = TempDir::new("overlay-wire-off");
    let dir = ByteBuffersDirectory::new();
    let fx = fixture(root.path(), &dir, &["a", "b"]);
    assert!(wired(&fx, false).is_none());
    assert_eq!(all(&fx), 2);
    assert!(!fx.service.has_overlay_source());
    assert!(wired(&fx, true).is_some());
    assert!(fx.service.has_overlay_source());
    assert!(wired(&fx, true).is_none(), "a source is installed already");
}
