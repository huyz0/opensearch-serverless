//! The collector's judge: a pack is judged only past 15 minutes of the
//! store's age, deleted only after two unreferenced sightings a minute
//! apart, and a referenced one is rechecked on a doubling backoff.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{PackSidecar, SidecarEpoch, node_pack_key};
use oss_writer::pack::{
    JUDGE_AGE_MILLIS, PackError, PackId, PackJudge, PackReferences, PackSidecarStore,
    SIGHTING_GAP_MILLIS,
};

const NODE: &str = "node-1";
const MIN: u64 = 60_000;

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

/// A pack at `ordinal` of epoch 1.
fn pack(e: &Emulator, ordinal: u64) {
    let key = node_pack_key("", NODE, 1, ordinal).expect("key");
    e.put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
        .expect("put");
}

/// A sidecar naming epoch 1 with `high` ordinals reserved.
fn sidecar(e: &Arc<Emulator>, high: u64) -> PackSidecarStore {
    let store = PackSidecarStore::new(dynstore(e), "", NODE).expect("store");
    let state = store.read().expect("read");
    store
        .update(state, |_| {
            PackSidecar::new(vec![SidecarEpoch::new(1, high)])
        })
        .expect("update");
    store
}

fn exists(e: &Emulator, ordinal: u64) -> bool {
    let key = node_pack_key("", NODE, 1, ordinal).expect("key");
    e.head(&key).expect("head").is_some()
}

fn marked(store: &PackSidecarStore, ordinal: u64) -> bool {
    store.read().expect("read").sidecar.epochs()[0].is_marked(ordinal)
}

/// Names the ordinals in its set; counts and can fail every question.
#[derive(Default)]
struct Names {
    named: Mutex<BTreeSet<u64>>,
    asked: AtomicUsize,
    broken: Mutex<bool>,
}

impl Names {
    fn naming(ordinals: &[u64]) -> Self {
        let n = Self::default();
        n.named.lock().unwrap().extend(ordinals.iter().copied());
        n
    }
    fn unname(&self, ordinal: u64) {
        self.named.lock().unwrap().remove(&ordinal);
    }
    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl PackReferences for Names {
    fn names(&self, pack: &PackId<'_>) -> Result<bool, PackError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if *self.broken.lock().unwrap() {
            return Err(PackError::Blob(oss_blob::BlobError::Unavailable));
        }
        assert_eq!((pack.node, pack.epoch), (NODE, 1));
        Ok(self.named.lock().unwrap().contains(&pack.ordinal))
    }
}

fn judge(e: &Arc<Emulator>, max_tracked: usize) -> PackJudge {
    PackJudge::new(dynstore(e), "", NODE, max_tracked).expect("judge")
}

#[test]
fn the_judging_age_and_sighting_gap_are_the_designs() {
    assert_eq!(JUDGE_AGE_MILLIS, 15 * MIN);
    assert_eq!(SIGHTING_GAP_MILLIS, MIN);
}

/// A pack younger than 15 minutes is not judged: no question to the
/// reference sources, nothing deleted.
#[test]
fn a_young_pack_is_not_judged() {
    let e = emulator();
    pack(&e, 0);
    let sc = sidecar(&e, 1);
    let names = Names::default();
    let mut j = judge(&e, 100);
    e.advance_clock(14 * MIN);
    let out = j.pass(&sc, &names, 0).expect("pass");
    assert_eq!((out.deleted, out.referenced), (0, 0));
    assert_eq!(names.asked(), 0);
    assert!(exists(&e, 0));
}

/// An old unreferenced pack is seen, then seen again a minute later, and
/// only then deleted and marked; the passes between cost no request.
#[test]
fn an_unreferenced_pack_is_deleted_at_its_second_sighting() {
    let e = emulator();
    pack(&e, 0);
    let sc = sidecar(&e, 1);
    let names = Names::default();
    let mut j = judge(&e, 100);
    e.advance_clock(16 * MIN);
    j.pass(&sc, &names, 1_000).expect("first sighting");
    assert!(exists(&e, 0), "one sighting deletes nothing");
    assert_eq!(names.asked(), 1);
    let heads = e.meter().count(S3Op::Head);
    j.pass(&sc, &names, 1_000 + MIN - 1).expect("too soon");
    assert_eq!(names.asked(), 1);
    assert_eq!(e.meter().count(S3Op::Head), heads, "nothing is due");
    let out = j.pass(&sc, &names, 1_000 + MIN).expect("second sighting");
    assert_eq!(out.deleted, 1);
    assert!(!exists(&e, 0));
    assert!(marked(&sc, 0));
    assert_eq!(e.meter().count(S3Op::Delete), 1);
}

/// A pack that is not in the store (a crash left its slot empty) is marked
/// collected without asking the reference sources.
#[test]
fn a_missing_pack_is_marked_without_a_question() {
    let e = emulator();
    let sc = sidecar(&e, 2);
    let names = Names::default();
    let mut j = judge(&e, 100);
    j.pass(&sc, &names, 0).expect("pass");
    assert!(marked(&sc, 0) && marked(&sc, 1));
    assert_eq!(names.asked(), 0);
}

/// A referenced pack is rechecked at 15 minutes, then at doubling intervals,
/// with no request between, and is deleted once nothing names it for two
/// sightings.
#[test]
fn a_referenced_pack_is_rechecked_on_a_doubling_backoff() {
    let e = emulator();
    pack(&e, 0);
    let sc = sidecar(&e, 1);
    let names = Names::naming(&[0]);
    let mut j = judge(&e, 100);
    e.advance_clock(16 * MIN);
    let mut now = 0;
    j.pass(&sc, &names, now).expect("first");
    assert_eq!(names.asked(), 1);
    // The first recheck is 15 minutes on, the next 30, then 60.
    for (gap, asked) in [(15 * MIN, 2), (30 * MIN, 3), (60 * MIN, 4)] {
        let heads = e.meter().count(S3Op::Head);
        j.pass(&sc, &names, now + gap - 1).expect("early");
        assert_eq!(names.asked(), asked - 1, "not due at {gap} - 1");
        assert_eq!(e.meter().count(S3Op::Head), heads, "no request while idle");
        now += gap;
        j.pass(&sc, &names, now).expect("due");
        assert_eq!(names.asked(), asked, "due at {gap}");
    }
    assert!(exists(&e, 0));
    // A merge dropped the last block: the next recheck sees it free, and the
    // second sighting a minute on deletes.
    names.unname(0);
    now += 120 * MIN;
    j.pass(&sc, &names, now).expect("first free sighting");
    assert!(exists(&e, 0));
    let out = j.pass(&sc, &names, now + MIN).expect("second");
    assert_eq!(out.deleted, 1);
    assert!(!exists(&e, 0));
}

/// A source that cannot be read stops the pass with an error and deletes
/// nothing.
#[test]
fn a_source_that_cannot_be_read_stops_judging() {
    let e = emulator();
    pack(&e, 0);
    pack(&e, 1);
    let sc = sidecar(&e, 2);
    let names = Names::default();
    *names.broken.lock().unwrap() = true;
    let mut j = judge(&e, 100);
    e.advance_clock(16 * MIN);
    assert!(j.pass(&sc, &names, 0).is_err());
    assert_eq!(names.asked(), 1, "it stops at the first");
    assert!(exists(&e, 0) && exists(&e, 1));
    assert!(!marked(&sc, 0));
}

/// A restarted judge starts every live pack's backoff again.
#[test]
fn a_restart_judges_each_live_pack_again() {
    let e = emulator();
    pack(&e, 0);
    let sc = sidecar(&e, 1);
    let names = Names::naming(&[0]);
    e.advance_clock(16 * MIN);
    judge(&e, 100).pass(&sc, &names, 0).expect("first");
    judge(&e, 100)
        .pass(&sc, &names, 1)
        .expect("after a restart");
    assert_eq!(names.asked(), 2);
}

/// Tracked packs are bounded: past the cap a pack is left for a later pass
/// and the pass says so.
#[test]
fn tracking_stops_at_the_cap_and_says_so() {
    let e = emulator();
    pack(&e, 0);
    pack(&e, 1);
    let sc = sidecar(&e, 2);
    let names = Names::naming(&[0, 1]);
    let mut j = judge(&e, 1);
    e.advance_clock(16 * MIN);
    let out = j.pass(&sc, &names, 0).expect("pass");
    assert_eq!(out.referenced, 1);
    assert!(out.at_cap);
    assert_eq!(names.asked(), 1);
}
