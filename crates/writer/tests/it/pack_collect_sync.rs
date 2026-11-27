//! The collector builds its sidecar from the writer's ledger and advances
//! the ledger as packs are collected: a node whose writer only ever kept a
//! ledger is still judged, and a restart re-reads only what is left.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::node_pack_key;
use oss_writer::pack::{
    PackCollector, PackError, PackId, PackIndexStore, PackLedger, PackReferences, PackSidecarStore,
    WINDOW_MILLIS, collector_for, window_of,
};

const MIN: u64 = 60_000;
const NODE: &str = "writer-1";

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn ledger(e: &Arc<Emulator>) -> PackLedger {
    PackLedger::new(dynstore(e), "", NODE).expect("ledger")
}

/// A pack at `ordinal` of `epoch`.
fn pack(e: &Emulator, epoch: u64, ordinal: u64) {
    let key = node_pack_key("", NODE, epoch, ordinal).expect("key");
    e.put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
        .expect("put");
}

fn exists(e: &Emulator, epoch: u64, ordinal: u64) -> bool {
    let key = node_pack_key("", NODE, epoch, ordinal).expect("key");
    e.head(&key).expect("head").is_some()
}

/// The node is in the index and its ledger holds `epoch` reserved to `high`.
fn writer_with_ledger(e: &Arc<Emulator>, epoch: u64, high: u64) {
    PackIndexStore::new(dynstore(e), "")
        .add(NODE, 1)
        .expect("indexed");
    ledger(e).reserve(epoch, high).expect("reserved");
}

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

/// Four other members, and the writer too when its lease is live.
fn members(writer_alive: bool) -> Vec<String> {
    let mut all: Vec<String> = (0..4).map(|n| format!("node-{n}")).collect();
    if writer_alive {
        all.push(NODE.to_owned());
    }
    all
}

/// Runs window `w` on the member elected for it.
fn run_as(c: &mut PackCollector, w: u64, refs: &Nothing, writer_alive: bool) {
    let now = w.saturating_mul(WINDOW_MILLIS).saturating_add(MIN);
    assert_eq!(window_of(now), w);
    let all = members(writer_alive);
    let me = collector_for(w, &all).expect("elected").to_owned();
    c.tick(&me, &all, now, refs).expect("tick");
}

/// A pass with the writer's lease gone.
fn run(c: &mut PackCollector, w: u64, refs: &Nothing) {
    run_as(c, w, refs, false);
}

/// A writer that kept only a ledger is judged: packs of an epoch reserved to
/// 16 are deleted at the second window, the ordinals never written are
/// marked, and the ledger's `collected_below` advances over them.
#[test]
fn a_node_with_only_a_ledger_is_judged_and_its_ledger_advances() {
    let e = emulator();
    writer_with_ledger(&e, 1, 16);
    pack(&e, 1, 0);
    pack(&e, 1, 1);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_as(&mut c, 100, &refs, true);
    assert!(exists(&e, 1, 0), "seen once, not deleted");
    run_as(&mut c, 101, &refs, true);
    assert!(!exists(&e, 1, 0) && !exists(&e, 1, 1));
    let epochs = ledger(&e).read().expect("ledger").epochs;
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].collected_below, 8, "the judged block, no further");
    assert_eq!(epochs[0].high, 16, "the writer's reservation is untouched");
}

/// The newest epoch's last reserved block may hold packs not yet written, so
/// it is not judged while the writer may be alive: an old unnamed pack there
/// is kept.
#[test]
fn the_newest_epochs_reserved_block_is_not_judged() {
    let e = emulator();
    writer_with_ledger(&e, 1, 16);
    pack(&e, 1, 9);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    for w in 100..103 {
        run_as(&mut c, w, &refs, true);
    }
    assert!(exists(&e, 1, 9));
    assert_eq!(refs.asked.load(Ordering::SeqCst), 0, "never asked about it");
}

/// An epoch the writer moved past is judged to its full `high`, and once all
/// of it is collected the ledger drops it while the newest stays.
#[test]
fn a_finished_epoch_is_judged_whole_and_then_dropped_from_the_ledger() {
    let e = emulator();
    PackIndexStore::new(dynstore(&e), "")
        .add(NODE, 1)
        .expect("indexed");
    ledger(&e).reserve(1, 8).expect("first");
    ledger(&e).reserve(2, 8).expect("second");
    pack(&e, 1, 7);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run(&mut c, 100, &refs);
    run(&mut c, 101, &refs);
    assert!(!exists(&e, 1, 7));
    let epochs = ledger(&e).read().expect("ledger").epochs;
    assert_eq!(epochs.iter().map(|x| x.epoch).collect::<Vec<_>>(), [2]);
}

/// A dead writer's newest epoch is judged to its full `high` too, and a node
/// with nothing left to judge and no lease leaves the index.
#[test]
fn a_dead_node_with_nothing_left_leaves_the_index() {
    let e = emulator();
    writer_with_ledger(&e, 1, 8);
    pack(&e, 1, 7);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run(&mut c, 100, &refs);
    run(&mut c, 101, &refs);
    assert!(
        !exists(&e, 1, 7),
        "the whole block is judged for a dead node"
    );
    run(&mut c, 102, &refs);
    let slot = oss_format::pack_index_slot(NODE);
    let state = PackIndexStore::new(dynstore(&e), "")
        .read(slot)
        .expect("index");
    assert!(!state.index.contains(NODE));
}

/// A live node is never dropped from the index for having nothing to judge.
#[test]
fn a_live_node_stays_in_the_index() {
    let e = emulator();
    writer_with_ledger(&e, 1, 8);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_as(&mut c, 100, &refs, true);
    let slot = oss_format::pack_index_slot(NODE);
    let state = PackIndexStore::new(dynstore(&e), "")
        .read(slot)
        .expect("index");
    assert!(state.index.contains(NODE));
}

/// After everything below `collected_below` is collected, a restarted
/// collector reads the ledger and sidecar once and asks the store about no
/// pack: the marks it needs are rebuilt from the ledger.
#[test]
fn a_restart_after_collection_reads_two_registers_and_no_pack() {
    let e = emulator();
    writer_with_ledger(&e, 1, 16);
    pack(&e, 1, 0);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut first = PackCollector::new(dynstore(&e), "", 100);
    run(&mut first, 100, &refs);
    run(&mut first, 101, &refs);
    assert!(!exists(&e, 1, 0));
    let heads = e.meter().count(S3Op::Head);
    let lists = e.meter().count(S3Op::List);
    let asked = refs.asked.load(Ordering::SeqCst);
    let mut restarted = PackCollector::new(dynstore(&e), "", 100);
    run(&mut restarted, 102, &refs);
    assert_eq!(e.meter().count(S3Op::Head), heads, "no pack is asked about");
    assert_eq!(e.meter().count(S3Op::List), lists);
    assert_eq!(refs.asked.load(Ordering::SeqCst), asked);
    let sidecar = PackSidecarStore::new(dynstore(&e), "", NODE).expect("sidecar");
    assert_eq!(sidecar.read().expect("read").sidecar.epochs().len(), 1);
}

/// Names the ordinals it is given.
struct Naming(Vec<u64>);

impl PackReferences for Naming {
    fn names(&self, pack: &PackId<'_>) -> Result<bool, PackError> {
        Ok(self.0.contains(&pack.ordinal))
    }
}

fn run_naming(c: &mut PackCollector, w: u64, refs: &dyn PackReferences, alive: bool) {
    let now = w.saturating_mul(WINDOW_MILLIS).saturating_add(MIN);
    let all = members(alive);
    let me = collector_for(w, &all).expect("elected").to_owned();
    c.tick(&me, &all, now, refs).expect("tick");
}

/// A ledger with nothing left to judge makes no sidecar.
#[test]
fn a_ledger_with_nothing_to_judge_makes_no_sidecar() {
    let e = emulator();
    writer_with_ledger(&e, 1, 8);
    ledger(&e)
        .update(ledger(&e).read().expect("read"), |epochs| {
            epochs
                .iter()
                .map(|x| oss_format::PackEpoch {
                    collected_below: x.high,
                    ..*x
                })
                .collect()
        })
        .expect("collected");
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run(&mut c, 100, &Nothing::default());
    let sidecar = PackSidecarStore::new(dynstore(&e), "", NODE).expect("sidecar");
    assert!(sidecar.read().expect("read").sidecar.epochs().is_empty());
}

/// A node judged while its lease was gone keeps the full `high` it was
/// judged to when it comes back: its still-named pack stays tracked.
#[test]
fn a_returning_node_keeps_the_high_it_was_judged_to() {
    let e = emulator();
    writer_with_ledger(&e, 1, 16);
    pack(&e, 1, 3);
    e.advance_clock(16 * MIN);
    let refs = Naming(vec![3]);
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_naming(&mut c, 100, &refs, false);
    run_naming(&mut c, 101, &refs, true);
    let sidecar = PackSidecarStore::new(dynstore(&e), "", NODE).expect("sidecar");
    let read = sidecar.read().expect("read");
    assert_eq!(read.sidecar.epochs()[0].high, 16);
    assert!(exists(&e, 1, 3), "a named pack is kept");
}
