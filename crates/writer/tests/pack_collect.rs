//! A packing node with collection on: the ledger's `high` is reserved ahead,
//! an epoch rotates every 1,024 ordinals, the live epochs are capped, and
//! the sidecar register is read and swapped.

mod common;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{
    NodePackLedger, PackEpoch, PackSidecar, SidecarEpoch, node_pack_key, pack_index_slot,
};
use oss_writer::pack::{
    PackError, PackIndexStore, PackLedger, PackSidecarStore, PackStore, PackWriter, Part,
    RESERVE_AHEAD,
};

const NODE: &str = "node-1";
const BASE_EPOCH: u64 = 10_000;

fn part() -> Part {
    struct Bytes(SliceSource<'static>);
    impl oss_blob::ChunkSource for Bytes {
        fn len(&self) -> u64 {
            self.0.len()
        }
        fn fill(&mut self, buf: &mut [u8]) -> oss_blob::BlobResult<usize> {
            self.0.fill(buf)
        }
        fn rewind(&mut self) -> oss_blob::BlobResult<()> {
            self.0.rewind()
        }
    }
    Part {
        index_uuid: "u".into(),
        index_name: "i".into(),
        shard: 0,
        name: "_0.cfs".into(),
        source: Box::new(Bytes(SliceSource::new(b"segment"))),
    }
}

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn writer(e: &Arc<Emulator>, collect: bool) -> PackWriter {
    PackWriter::new(
        PackStore::new(dynstore(e), "", NODE),
        Some(PackLedger::new(dynstore(e), "", NODE).expect("ledger")),
        BASE_EPOCH,
        Duration::ZERO,
        8 << 20,
    )
    .with_collection(collect)
}

fn ledger(e: &Arc<Emulator>) -> Vec<PackEpoch> {
    let l = PackLedger::new(dynstore(e), "", NODE).expect("ledger");
    l.read().expect("reads").epochs
}

/// AC-2: with collection on, `high` is reserved ahead in blocks, so N packs
/// cost one ledger write a block, not one a pack, and `high` is never below
/// the ordinals used.
#[test]
fn high_is_reserved_ahead_in_blocks() {
    let e = emulator();
    let w = writer(&e, true);
    let blocks = 3u64;
    let packs = RESERVE_AHEAD * blocks;
    for _ in 0..packs {
        w.add(vec![part()]).expect("packs");
    }
    // One PUT a pack, plus the ledger: one create and a swap a further block.
    assert_eq!(
        e.meter().count(S3Op::Put),
        packs + blocks,
        "a ledger write a block of {RESERVE_AHEAD}"
    );
    let epochs = ledger(&e);
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].epoch, BASE_EPOCH);
    assert_eq!(epochs[0].high, packs, "high is the end of the last block");
}

/// With collection off the ledger is written once, before the first pack, as
/// today: `high` stays 0 and no block is reserved.
#[test]
fn off_the_ledger_is_written_once() {
    let e = emulator();
    let w = writer(&e, false);
    for _ in 0..(RESERVE_AHEAD * 2) {
        w.add(vec![part()]).expect("packs");
    }
    assert_eq!(e.meter().count(S3Op::Put), RESERVE_AHEAD * 2 + 1);
    assert_eq!(ledger(&e)[0].high, 0);
}

/// AC-2: an epoch rotates at 1,024 ordinals: the next pack is in the next
/// epoch at ordinal 0, the ledger holds both, and the first is full.
#[test]
fn an_epoch_rotates_every_1024_ordinals() {
    let e = emulator();
    let w = writer(&e, true);
    for _ in 0..1025 {
        w.add(vec![part()]).expect("packs");
    }
    let epochs = ledger(&e);
    assert_eq!(epochs.len(), 2, "{epochs:?}");
    assert_eq!(epochs[0].high, 1024);
    assert_eq!(epochs[1].epoch, BASE_EPOCH + 1);
    let key = node_pack_key("", NODE, BASE_EPOCH + 1, 0).expect("key");
    assert!(e.head(&key).expect("head").is_some(), "{key}");
    // The Java's reader parses what a Rust writer wrote.
    let mut buf = vec![0u8; 1 << 16];
    let read = e
        .read_register(&format!("packs/{NODE}/ledger"), &mut buf)
        .expect("reads")
        .expect("exists");
    let parsed = NodePackLedger::parse(&buf[..read.len]).expect("version 1");
    assert_eq!(parsed.epochs().len(), 2);
}

/// AC-3: at 1,500 live epochs a writer stops rotating and says so; the
/// publish fails rather than leaving a pack the collector cannot track.
#[test]
fn at_1500_live_epochs_the_writer_stops() {
    let e = emulator();
    let l = PackLedger::new(dynstore(&e), "", NODE).expect("ledger");
    let full: Vec<PackEpoch> = (0..1500)
        .map(|n| PackEpoch {
            epoch: n,
            collected_below: 0,
            high: 1024,
        })
        .collect();
    l.update(l.read().expect("reads"), |_| full.clone())
        .expect("planted");
    let w = writer(&e, true);
    let err = w.add(vec![part()]).expect_err("refused");
    assert!(matches!(err, PackError::TooManyEpochs), "{err}");
    assert!(err.to_string().contains("1500"), "{err}");
}

/// AC-3: the sidecar register is absent until written, then read back and
/// swapped by generation; a stale swap re-reads and re-applies.
#[test]
fn the_sidecar_is_read_and_swapped_by_generation() {
    let e = emulator();
    let s = PackSidecarStore::new(dynstore(&e), "", NODE).expect("store");
    let first = s.read().expect("reads");
    assert!(first.sidecar.epochs().is_empty());
    let mut one = SidecarEpoch::new(5, 9);
    one.mark(3);
    let written = s
        .update(first, |_| PackSidecar::new(vec![one]))
        .expect("created");
    let again = s.read().expect("reads");
    assert_eq!(again.sidecar.epochs().len(), 1);
    assert!(again.sidecar.epochs()[0].is_marked(3));
    assert_eq!(again.generation, written.generation);
    // A stale holder's swap loses, re-reads, and applies its change on top.
    let _ = s
        .update(again.clone(), |c| {
            let mut next = c.clone();
            next.epochs_mut().push(SidecarEpoch::new(6, 0));
            next
        })
        .expect("swapped");
    let merged = s
        .update(again, |c| {
            let mut next = c.clone();
            next.epochs_mut().push(SidecarEpoch::new(7, 0));
            next
        })
        .expect("re-applied");
    let epochs: Vec<u64> = merged.sidecar.epochs().iter().map(|e| e.epoch).collect();
    assert_eq!(epochs, vec![5, 6, 7]);
}

fn indexed(store: Arc<dyn BlobStore>, e: &Arc<Emulator>) -> PackWriter {
    PackWriter::new(
        PackStore::new(dynstore(e), "", NODE),
        Some(PackLedger::new(dynstore(e), "", NODE).expect("ledger")),
        BASE_EPOCH,
        Duration::ZERO,
        8 << 20,
    )
    .with_collection(true)
    .with_index(PackIndexStore::new(store, ""), NODE)
}

/// AC-4: the node is named in its index register before its first pack, in
/// the same publish: pack, ledger create and index create.
#[test]
fn the_node_is_indexed_before_its_first_pack() {
    let e = emulator();
    let w = indexed(dynstore(&e), &e);
    w.add(vec![part()]).expect("packs");
    assert_eq!(e.meter().count(S3Op::Put), 3, "pack, ledger, index");
    let index = PackIndexStore::new(dynstore(&e), "");
    let state = index.read(pack_index_slot(NODE)).expect("reads");
    assert_eq!(state.index.stamp(NODE), Some(BASE_EPOCH));
    // Later packs of the epoch add nothing to the index.
    let puts = e.meter().count(S3Op::Put);
    w.add(vec![part()]).expect("packs");
    assert_eq!(e.meter().count(S3Op::Put), puts + 1, "only the pack");
}

/// AC-4: a rotation names the node again, with the new epoch's stamp.
#[test]
fn a_rotation_adds_the_node_again() {
    let e = emulator();
    let w = indexed(dynstore(&e), &e);
    for _ in 0..1025 {
        w.add(vec![part()]).expect("packs");
    }
    let state = PackIndexStore::new(dynstore(&e), "")
        .read(pack_index_slot(NODE))
        .expect("reads");
    assert_eq!(state.index.stamp(NODE), Some(BASE_EPOCH + 1));
}

/// AC-4: when the index cannot be written the publish fails and no pack is
/// written, so no pack exists that the index cannot find.
#[test]
fn no_pack_is_written_when_the_index_add_fails() {
    let e = emulator();
    PackIndexStore::new(dynstore(&e), "")
        .add(NODE, 1)
        .expect("seeded");
    let busy = common::Hooked::swapping(&e, |_, _, _, _| {
        Ok(oss_blob::register::SwapOutcome::Conflict(None))
    });
    let w = indexed(busy, &e);
    let err = w.add(vec![part()]).expect_err("fails");
    assert!(matches!(err, PackError::IndexBusy), "{err}");
    let key = node_pack_key("", NODE, BASE_EPOCH, 0).expect("key");
    assert!(e.head(&key).expect("head").is_none(), "no pack was written");
}
