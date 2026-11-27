//! The collector's tick: one elected node a window runs a pass over the
//! nodes the pack index names, judges each, and drops a node with no live
//! epoch from the index.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{PackSidecar, SidecarEpoch, node_pack_key};
use oss_writer::pack::{
    PackCollector, PackError, PackId, PackIndexStore, PackReferences, PackSidecarStore,
    WINDOW_MILLIS, collector_for, window_of,
};

const MIN: u64 = 60_000;

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

/// A node with its pack at ordinal 0 of epoch 1, named in the index.
fn node_with_a_pack(e: &Arc<Emulator>, node: &str) -> PackSidecarStore {
    let key = node_pack_key("", node, 1, 0).expect("key");
    e.put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
        .expect("put");
    PackIndexStore::new(dynstore(e), "")
        .add(node, 1)
        .expect("indexed");
    let sidecar = PackSidecarStore::new(dynstore(e), "", node).expect("sidecar");
    let state = sidecar.read().expect("read");
    sidecar
        .update(state, |_| PackSidecar::new(vec![SidecarEpoch::new(1, 1)]))
        .expect("update");
    sidecar
}

fn exists(e: &Emulator, node: &str) -> bool {
    let key = node_pack_key("", node, 1, 0).expect("key");
    e.head(&key).expect("head").is_some()
}

/// Names nothing; counts the questions.
#[derive(Default)]
struct Nothing {
    asked: AtomicUsize,
}

impl PackReferences for Nothing {
    fn names(&self, _: &PackId<'_>) -> Result<bool, PackError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
}

struct Broken;

impl PackReferences for Broken {
    fn names(&self, _: &PackId<'_>) -> Result<bool, PackError> {
        Err(PackError::Unreadable("a test source"))
    }
}

fn members() -> Vec<String> {
    (0..4).map(|n| format!("node-{n}")).collect()
}

/// A time inside window `w`, and the member elected for it.
fn window(w: u64) -> (u64, String) {
    let now = w.saturating_mul(WINDOW_MILLIS).saturating_add(MIN);
    assert_eq!(window_of(now), w);
    let elected = collector_for(w, &members()).expect("elected").to_owned();
    (now, elected)
}

fn collector(e: &Arc<Emulator>) -> PackCollector {
    PackCollector::new(dynstore(e), "", 100)
}

#[test]
fn only_the_elected_node_runs_a_windows_pass() {
    let e = emulator();
    node_with_a_pack(&e, "writer");
    let (now, elected) = window(100);
    let other = members()
        .into_iter()
        .find(|m| *m != elected)
        .expect("other");
    let before = e.meter().count(S3Op::Get);
    let out = collector(&e)
        .tick(&other, &members(), now, &Nothing::default())
        .expect("tick");
    assert!(!out.ran);
    assert_eq!(e.meter().count(S3Op::Get), before, "no request");
    let out = collector(&e)
        .tick(&elected, &members(), now, &Nothing::default())
        .expect("tick");
    assert!(out.ran);
}

#[test]
fn a_window_runs_once_whatever_the_ticks() {
    let e = emulator();
    node_with_a_pack(&e, "writer");
    let (now, elected) = window(100);
    let mut c = collector(&e);
    assert!(
        c.tick(&elected, &members(), now, &Nothing::default())
            .expect("a")
            .ran
    );
    let before = e.meter().count(S3Op::Get);
    assert!(
        !c.tick(
            &elected,
            &members(),
            now.saturating_add(MIN),
            &Nothing::default()
        )
        .expect("b")
        .ran
    );
    assert_eq!(e.meter().count(S3Op::Get), before);
}

#[test]
fn a_pack_nothing_names_goes_at_the_collectors_second_window() {
    let e = emulator();
    let sidecar = node_with_a_pack(&e, "writer");
    let refs = Nothing::default();
    let mut c = collector(&e);
    // The pack is 15 minutes old by the store's clock before it is judged.
    e.advance_clock(16_u64.saturating_mul(MIN));
    // Two windows the same node is elected for: its memory holds the sighting.
    let (first, elected) = window(100);
    let second = (101..200)
        .find(|w| collector_for(*w, &members()) == Some(elected.as_str()))
        .expect("the node is elected again");
    assert_eq!(
        c.tick(&elected, &members(), first, &refs)
            .expect("one")
            .deleted,
        0
    );
    assert!(exists(&e, "writer"), "seen once");
    let out = c
        .tick(
            &elected,
            &members(),
            second.saturating_mul(WINDOW_MILLIS).saturating_add(MIN),
            &refs,
        )
        .expect("two");
    assert_eq!(out.deleted, 1);
    assert!(!exists(&e, "writer"));
    assert!(sidecar.read().expect("read").sidecar.epochs()[0].is_marked(0));
}

#[test]
fn a_source_that_cannot_be_read_stops_the_pass_with_an_error() {
    let e = emulator();
    node_with_a_pack(&e, "writer");
    e.advance_clock(16_u64.saturating_mul(MIN));
    let (now, elected) = window(100);
    assert!(
        collector(&e)
            .tick(&elected, &members(), now, &Broken)
            .is_err()
    );
    assert!(exists(&e, "writer"));
}

#[test]
fn a_node_with_no_live_epoch_is_dropped_from_the_index_and_one_with_one_is_not() {
    let e = emulator();
    node_with_a_pack(&e, "live");
    // A node whose sidecar holds no epoch at all.
    PackIndexStore::new(dynstore(&e), "")
        .add("finished", 1)
        .expect("indexed");
    let (now, elected) = window(100);
    let out = collector(&e)
        .tick(&elected, &members(), now, &Nothing::default())
        .expect("tick");
    assert_eq!(out.dropped, 1);
    let index = PackIndexStore::new(dynstore(&e), "");
    let named: Vec<String> = (0..16u8)
        .flat_map(|slot| {
            let state = index.read(slot).expect("read");
            state.index.nodes().map(str::to_owned).collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(named, vec!["live".to_owned()]);
}

#[test]
fn an_idle_pass_writes_and_lists_nothing() {
    let e = emulator();
    node_with_a_pack(&e, "writer");
    let (now, elected) = window(100);
    let mut c = collector(&e);
    c.tick(&elected, &members(), now, &Nothing::default())
        .expect("first");
    let puts = e.meter().count(S3Op::Put);
    let lists = e.meter().count(S3Op::List);
    c.tick(
        &elected,
        &members(),
        now.saturating_add(WINDOW_MILLIS),
        &Nothing::default(),
    )
    .expect("idle");
    assert_eq!(e.meter().count(S3Op::Put), puts);
    assert_eq!(e.meter().count(S3Op::List), lists);
}

/// A node with more live packs than the tracking cap says so, and the pass
/// still judges the ones it tracks.
#[test]
fn a_tick_reports_that_tracking_stopped_at_its_cap() {
    let e = emulator();
    let sidecar = node_with_a_pack(&e, "writer");
    let second = node_pack_key("", "writer", 1, 1).expect("key");
    e.put(&second, &mut SliceSource::new(b"pack"), &mut [0; 16])
        .expect("put");
    let state = sidecar.read().expect("read");
    sidecar
        .update(state, |_| PackSidecar::new(vec![SidecarEpoch::new(1, 2)]))
        .expect("update");
    e.advance_clock(16_u64.saturating_mul(MIN));
    let (now, elected) = window(100);
    let mut capped = PackCollector::new(dynstore(&e), "", 1);
    let out = capped
        .tick(&elected, &members(), now, &Nothing::default())
        .expect("tick");
    assert!(out.at_cap);
    assert!(
        !collector(&e)
            .tick(&elected, &members(), now, &Nothing::default())
            .expect("tick")
            .at_cap
    );
}
