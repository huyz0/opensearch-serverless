//! Node packs: the store, the batching writer and the ledger, over the
//! in-process emulator with its request meter.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, ByteRange, Emulator, S3Op, SliceSource};
use oss_format::{NodePackHeader, ShardRef, node_pack_key};
use oss_writer::pack::{PackError, PackLedger, PackStore, PackWriter, Part};

const NODE: &str = "node-1";

/// A part of `bytes` whose source can be rewound.
fn part(index: &str, shard: u32, name: &str, bytes: &'static [u8]) -> Part {
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
        index_uuid: format!("{index}-uuid"),
        index_name: index.to_owned(),
        shard,
        name: name.to_owned(),
        source: Box::new(Bytes(SliceSource::new(bytes))),
    }
}

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

fn read(e: &Emulator, key: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    e.get(
        key,
        ByteRange::Full,
        &mut buf,
        &mut |chunk: &[u8]| -> oss_blob::BlobResult<()> {
            out.extend_from_slice(chunk);
            Ok(())
        },
    )
    .expect("reads");
    out
}

fn writer(e: &Arc<Emulator>, epoch: u64, linger_ms: u64, max: u64) -> PackWriter {
    PackWriter::new(
        PackStore::new(dynstore(e), "", NODE),
        Some(PackLedger::new(dynstore(e), "", NODE).expect("ledger")),
        epoch,
        Duration::from_millis(linger_ms),
        max,
    )
}

/// AC-1: parts of several shards are one PUT whose bytes are the header and
/// the parts, and a landed range resolves to the part's bytes.
#[test]
fn several_parts_are_one_put_and_each_landing_names_its_bytes() {
    let e = emulator();
    let store = PackStore::new(dynstore(&e), "", NODE);
    let landed = store
        .write(
            9,
            0,
            vec![
                part("a", 0, "_0.seg", b"AAAA"),
                part("b", 3, "_1.seg", b"BB"),
            ],
        )
        .expect("written");
    assert_eq!(e.meter().count(S3Op::Put), 1);
    let key = node_pack_key("", NODE, 9, 0).unwrap();
    let pack = read(&e, &key);
    let header = NodePackHeader::parse(&pack).expect("a pack header");
    assert_eq!(header.blocks().len(), 2);
    assert_eq!(header.blocks()[1].shard, 3);
    assert_eq!(header.blocks()[1].index_name, "b");
    assert_eq!(landed[0].offset, header.body_start);
    for (one, bytes) in landed.iter().zip([&b"AAAA"[..], b"BB"]) {
        let start = usize::try_from(one.offset).unwrap();
        assert_eq!(&pack[start..start + bytes.len()], bytes);
    }
    // A range of a block names the pack, and a range past it names nothing.
    let at = landed[0].file_at(1, 2).expect("inside");
    assert_eq!(
        at.encode(),
        format!("@{NODE}/e={:020}/{:020}#{}:2", 9, 0, landed[0].offset + 1)
    );
    assert!(landed[0].file_at(3, 2).is_none());
    assert!(landed[0].file_at(u64::MAX, 2).is_none());
    assert!(landed[0].file_at(0, 4).is_some());
}

/// AC-2: a taken slot is refused and nothing is overwritten.
#[test]
fn a_taken_slot_is_refused_and_keeps_its_bytes() {
    let e = emulator();
    let store = PackStore::new(dynstore(&e), "", NODE);
    store
        .write(1, 0, vec![part("a", 0, "_0.seg", b"first")])
        .unwrap();
    let key = node_pack_key("", NODE, 1, 0).unwrap();
    let before = read(&e, &key);
    let again = store.write(1, 0, vec![part("a", 0, "_0.seg", b"second")]);
    assert!(matches!(again, Err(PackError::Taken)), "{again:?}");
    assert_eq!(read(&e, &key), before);
}

/// AC-3: publishes within the linger share a pack, one PUT; a lone publish is
/// its own pack after the linger; a full batch does not wait.
#[test]
fn publishes_within_the_linger_share_one_pack() {
    let e = emulator();
    let w = Arc::new(writer(&e, 5, 400, 1 << 20));
    let handles: Vec<_> = (0..4u32)
        .map(|n| {
            let w = Arc::clone(&w);
            std::thread::spawn(move || {
                w.add(vec![part("a", n, "_0.seg", b"xxxx")])
                    .expect("landed")
                    .remove(0)
            })
        })
        .collect();
    let landed: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    // One PUT of the pack and one create of the ledger register.
    assert_eq!(e.meter().count(S3Op::Put), 2, "one pack");
    assert!(landed.iter().all(|l| l.ordinal == 0 && l.epoch == 5));
    let mut offsets: Vec<u64> = landed.iter().map(|l| l.offset).collect();
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), 4, "each its own block");
    assert_eq!(w.next_ordinal(), 1);
}

#[test]
fn a_lone_publish_is_its_own_pack_after_the_linger() {
    let e = emulator();
    let w = writer(&e, 5, 30, 1 << 20);
    let began = std::time::Instant::now();
    let first = w.add(vec![part("a", 0, "_0.seg", b"x")]).unwrap();
    assert!(began.elapsed() >= Duration::from_millis(30));
    let second = w.add(vec![part("a", 0, "_1.seg", b"y")]).unwrap();
    assert_eq!((first[0].ordinal, second[0].ordinal), (0, 1));
    // Two packs and the ledger's create.
    assert_eq!(e.meter().count(S3Op::Put), 3);
    assert!(w.add(Vec::new()).unwrap().is_empty());
}

#[test]
fn a_full_batch_is_written_without_waiting_out_the_linger() {
    let e = emulator();
    let w = writer(&e, 5, 60_000, 4);
    let began = std::time::Instant::now();
    w.add(vec![part("a", 0, "_0.seg", b"12345")]).unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "did not wait the linger"
    );
}

/// AC-4: a failed pack fails every publish in it and spends its ordinal.
#[test]
fn a_failed_pack_fails_its_publishes_and_the_ordinal_is_spent() {
    let e = emulator();
    let w = Arc::new(writer(&e, 5, 300, 1 << 20));
    // The ledger write (a register create) comes first; fail the PUT of the pack.
    w.add(vec![part("a", 0, "_0.seg", b"ok")]).unwrap();
    e.fail_next(S3Op::Put, 8);
    let handles: Vec<_> = (0..2u32)
        .map(|n| {
            let w = Arc::clone(&w);
            std::thread::spawn(move || w.add(vec![part("a", n, "_1.seg", b"zz")]))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().all(Result::is_err), "{results:?}");
    assert_eq!(w.next_ordinal(), 2, "the failed pack spent its ordinal");
}

/// AC-5: the epoch is recorded in the ledger before the first pack, once.
#[test]
fn the_epoch_is_recorded_once_before_the_first_pack() {
    let e = emulator();
    let ledger = PackLedger::new(dynstore(&e), "", NODE).unwrap();
    assert!(ledger.read().unwrap().epochs.is_empty());
    let w = writer(&e, 77, 1, 1 << 20);
    w.add(vec![part("a", 0, "_0.seg", b"x")]).unwrap();
    let after_first = ledger.read().unwrap();
    assert_eq!(after_first.epochs.len(), 1);
    assert_eq!(after_first.epochs[0].epoch, 77);
    let puts = e.meter().count(S3Op::Put);
    w.add(vec![part("a", 0, "_1.seg", b"y")]).unwrap();
    assert_eq!(
        ledger.read().unwrap(),
        after_first,
        "no ledger write a pack"
    );
    assert_eq!(e.meter().count(S3Op::Put), puts + 1);
    // A second writer of the node adds its epoch beside the first.
    let second = writer(&e, 78, 1, 1 << 20);
    second.add(vec![part("a", 0, "_0.seg", b"x")]).unwrap();
    let epochs: Vec<u64> = ledger
        .read()
        .unwrap()
        .epochs
        .iter()
        .map(|x| x.epoch)
        .collect();
    assert_eq!(epochs, vec![77, 78]);
}

/// AC-6: two writers racing to record their epochs both end up in the ledger.
#[test]
fn racing_epoch_records_both_land() {
    let e = emulator();
    let ledgers: Vec<_> = (0..6u64)
        .map(|n| {
            let ledger = PackLedger::new(dynstore(&e), "", NODE).unwrap();
            std::thread::spawn(move || ledger.add_epoch(n).expect("recorded"))
        })
        .collect();
    for h in ledgers {
        h.join().unwrap();
    }
    let ledger = PackLedger::new(dynstore(&e), "", NODE).unwrap();
    let mut epochs: Vec<u64> = ledger
        .read()
        .unwrap()
        .epochs
        .iter()
        .map(|x| x.epoch)
        .collect();
    epochs.sort_unstable();
    assert_eq!(epochs, (0..6).collect::<Vec<_>>());
    // Recording one already there writes nothing.
    let puts = e.meter().count(S3Op::Put);
    ledger.add_epoch(3).unwrap();
    assert_eq!(e.meter().count(S3Op::Put), puts);
}

/// A shard's manifest location for a landed block resolves in `ShardRef`.
#[test]
fn a_landed_location_resolves_to_the_pack_key() {
    let e = emulator();
    let store = PackStore::new(dynstore(&e), "", NODE);
    let landed = store
        .write(4, 2, vec![part("a", 0, "_0.seg", b"abcdef")])
        .unwrap();
    let location = landed[0].file_at(2, 3).unwrap();
    let shard = ShardRef {
        index: "a",
        uuid: "a-uuid",
        shard: 0,
    };
    let (key, offset) = shard.blob_key("", "_0.cfs", &location).unwrap();
    assert_eq!(key, node_pack_key("", NODE, 4, 2).unwrap());
    assert_eq!(offset, landed[0].offset + 2);
}

/// AC-8: a clock that stepped back cannot reuse an epoch the ledger holds.
#[test]
fn the_next_epoch_is_past_every_epoch_in_the_ledger() {
    let e = emulator();
    let ledger = PackLedger::new(dynstore(&e), "", NODE).unwrap();
    assert_eq!(ledger.next_epoch(100).unwrap(), 100, "an empty ledger");
    ledger.add_epoch(500).unwrap();
    assert_eq!(
        ledger.next_epoch(100).unwrap(),
        501,
        "the clock stepped back"
    );
    assert_eq!(ledger.next_epoch(900).unwrap(), 900, "the clock is ahead");
    assert_eq!(ledger.next_epoch(500).unwrap(), 501, "the same millisecond");
}

/// AC-9: a leader that fails while writing leaves no follower waiting.
#[test]
fn a_leader_that_panics_leaves_its_followers_an_error_not_a_wait() {
    struct Boom;
    impl oss_blob::ChunkSource for Boom {
        fn len(&self) -> u64 {
            4
        }
        fn fill(&mut self, _: &mut [u8]) -> oss_blob::BlobResult<usize> {
            panic!("a source failed");
        }
        fn rewind(&mut self) -> oss_blob::BlobResult<()> {
            Ok(())
        }
    }
    let e = emulator();
    let w = Arc::new(writer(&e, 5, 300, 1 << 20));
    let leader = {
        let w = Arc::clone(&w);
        std::thread::spawn(move || {
            let mut bad = part("a", 0, "_0.seg", b"xxxx");
            bad.source = Box::new(Boom);
            w.add(vec![bad])
        })
    };
    // The follower joins the leader's batch inside its linger.
    std::thread::sleep(Duration::from_millis(100));
    let follower = {
        let w = Arc::clone(&w);
        std::thread::spawn(move || w.add(vec![part("a", 1, "_1.seg", b"yyyy")]))
    };
    assert!(leader.join().is_err(), "the leader panicked");
    assert!(
        follower.join().expect("did not panic").is_err(),
        "an error, not a wait"
    );
}
