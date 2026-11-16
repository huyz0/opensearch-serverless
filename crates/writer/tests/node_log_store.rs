//! A node's write log: one blob per window holding every shard, an index in
//! each header, fences and deletes by name (the shell's `NodeLogStoreTests`).

mod common;

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use common::{blob, count, emulated, ids, record, window};
use oss_blob::{BlobStore, FsStore, S3Op};
use oss_format::{NodeLogEntry, OrdinalRange, ShardBatch, ShardKey, WalRecord};
use oss_testkit::TempDir;
use oss_writer::log::{LogError, NodeLogStore};

fn key(uuid: &str, shard: u32) -> ShardKey {
    ShardKey::new(uuid, shard, None::<&str>)
}

fn header(entry: Option<NodeLogEntry<'static>>) -> oss_format::NodeLogHeader<'static> {
    match entry {
        Some(NodeLogEntry::Records(h)) => h,
        other => panic!("expected a header, got {other:?}"),
    }
}

#[test]
fn one_blob_holds_every_shard_and_each_slice_reads_alone() {
    let (store, log) = emulated();
    let (a0, a1, b0) = (key("uuid-a", 0), key("uuid-a", 1), key("uuid-b", 0));
    let ra = [record("a", 7), record("b", 8)];
    let rb = [record("c", 1)];
    let rd = [WalRecord::deletion("d", 4, 2, 2).encode().expect("encodes")];
    let encoded = window(
        0,
        &[
            ShardBatch {
                shard: &a0,
                term: 2,
                records: &ra,
            },
            ShardBatch {
                shard: &b0,
                term: 5,
                records: &rb,
            },
            ShardBatch {
                shard: &a1,
                term: 2,
                records: &rd,
            },
        ],
    );
    log.append(3, 0, &encoded.bytes).expect("lands");
    assert_eq!(
        count(&store, S3Op::Put),
        1,
        "a window of three shards is one PUT"
    );

    let got = header(log.read_entry(3, 0).expect("reads"));
    assert_eq!(
        count(&store, S3Op::GetRange),
        1,
        "a header is one ranged read"
    );
    assert_eq!(got, encoded.header);
    assert_eq!(got.slices.len(), 3);
    assert_eq!(got.ordinals_of(&b0), &[OrdinalRange { from: 0, to: 0 }]);

    let first = got.slices_of(&a0).next().expect("a slice of a0").clone();
    assert_eq!(
        ids(&log.read_slice(3, &got, &first).expect("reads")),
        ["a", "b"]
    );
    assert_eq!((first.max_seq_no, first.term), (8, 2));
    assert_eq!(
        count(&store, S3Op::GetRange),
        2,
        "one shard's slice is one ranged read"
    );
    let deletions = log
        .read_slice(3, &got, got.slices_of(&a1).next().expect("a1"))
        .expect("reads");
    let deleted = oss_format::slice_records(&deletions)
        .next()
        .expect("one")
        .expect("parses");
    assert!(deleted.is_deletion());
    assert_eq!(
        ids(&log
            .read_slice(3, &got, got.slices_of(&b0).next().expect("b0"))
            .expect("reads")),
        ["c"]
    );

    let body = log.read_body(3, &got).expect("reads");
    assert_eq!(
        count(&store, S3Op::GetRange),
        5,
        "a whole body is one ranged read"
    );
    let b = NodeLogStore::parse_slice(&body, got.slices_of(&b0).next().expect("b0"))
        .expect("in the body");
    assert_eq!(ids(b), ["c"]);
    assert_eq!(count(&store, S3Op::List), 0);
}

#[test]
fn a_slice_not_in_the_header_or_past_the_body_is_refused() {
    let (_store, log) = emulated();
    let a0 = key("uuid-a", 0);
    let ra = [record("a", 1)];
    let encoded = window(
        0,
        &[ShardBatch {
            shard: &a0,
            term: 1,
            records: &ra,
        }],
    );
    log.append(3, 0, &encoded.bytes).expect("lands");
    let mut other = encoded.header.slices[0].clone();
    other.length = other.length.saturating_add(1);
    assert_eq!(
        log.read_slice(3, &encoded.header, &other),
        Err(LogError::OutOfRange("slice"))
    );
    assert_eq!(
        NodeLogStore::parse_slice(b"short", &other),
        Err(LogError::OutOfRange("slice"))
    );
}

#[test]
fn a_free_slot_reads_as_none_and_a_short_blob_is_refused() {
    let (store, log) = emulated();
    assert_eq!(log.read_entry(0, 0), Ok(None));
    assert_eq!(log.last_header_below(0, 5, 0), Ok(None));
    log.append(0, 0, b"NLGR\x02").expect("lands");
    assert!(matches!(log.read_entry(0, 0), Err(LogError::Short { .. })));
    log.append(0, 1, b"XXXXXXXXXXXX").expect("lands");
    assert!(matches!(log.read_entry(0, 1), Err(LogError::Format(_))));
    store.fail_next(S3Op::GetRange, 1);
    assert!(matches!(log.read_entry(0, 0), Err(LogError::Blob(_))));
}

#[test]
fn a_fence_stops_the_writer_and_every_blob_it_landed_is_below_it() {
    let (store, log) = emulated();
    for ordinal in 0..37 {
        log.append(1, ordinal, &blob(ordinal)).expect("lands");
    }
    let ops = [S3Op::Put, S3Op::List, S3Op::Head, S3Op::GetRange];
    let before = ops.map(|op| count(&store, op));
    let fence = log.fence(1, 5, "node-2", 9).expect("fences");
    let spent: Vec<u64> = ops
        .iter()
        .zip(before)
        .map(|(op, was)| count(&store, *op).saturating_sub(was))
        .collect();
    assert_eq!(fence, 37, "the fence takes the first free slot");
    // From the hint at 5: HEADs at 5, 6, 8, 12, 20, 36 and 68 double to a
    // free slot, then 52, 44, 40, 38 and 37 halve back to the first.
    assert_eq!(
        spent,
        [1, 0, 12, 1],
        "one PUT, no LIST, twelve HEADs, one ranged GET of the slot below"
    );
    assert!(matches!(
        log.read_entry(1, fence),
        Ok(Some(NodeLogEntry::Fence(_)))
    ));

    assert_eq!(log.append(1, 37, &blob(37)), Err(LogError::Taken));
    assert_eq!(
        header(
            log.last_header_below(1, fence, 0)
                .expect("reads")
                .map(NodeLogEntry::Records)
        )
        .ordinal,
        36
    );
    // A second successor shares the fence rather than adding one.
    assert_eq!(log.fence(1, 0, "node-3", 9), Ok(37));
    assert_eq!(log.read_entry(1, 38), Ok(None));
}

#[test]
fn first_free_doubles_then_halves_from_its_hint() {
    let (store, log) = emulated();
    assert_eq!(log.first_free(0, 0), Ok(0));
    for ordinal in 0..100 {
        log.append(0, ordinal, &blob(ordinal)).expect("lands");
    }
    let heads = count(&store, S3Op::Head);
    assert_eq!(log.first_free(0, 0), Ok(100));
    let spent = count(&store, S3Op::Head).saturating_sub(heads);
    assert!(spent <= 16, "logarithmic, not linear: {spent} HEADs");
    assert_eq!(log.first_free(0, 99), Ok(100));
    assert_eq!(log.first_free(0, 100), Ok(100));
}

#[test]
fn a_fence_racing_the_writer_lands_after_everything_the_writer_landed() {
    race_a_fence_against_the_writer(Duration::from_millis(1));
}

/// Back to back, the writer's next PUT is always headed for the first free
/// slot; the fence has to aim past it.
#[test]
fn a_fence_racing_the_writer_stops_a_writer_appending_back_to_back() {
    race_a_fence_against_the_writer(Duration::ZERO);
}

fn race_a_fence_against_the_writer(pause: Duration) {
    for round in 0..20 {
        let dir = TempDir::new("node-log-race");
        let fs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).expect("a store"));
        let log = NodeLogStore::new(fs, "", "node-1").expect("valid");
        let landed = Arc::new(AtomicI64::new(-1));
        let stopped = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));
        let started = Arc::new(Barrier::new(2));
        let writer = {
            let (log, landed, stopped, done, started) = (
                log.clone(),
                Arc::clone(&landed),
                Arc::clone(&stopped),
                Arc::clone(&done),
                Arc::clone(&started),
            );
            std::thread::spawn(move || {
                let mut waiting = true;
                for ordinal in 0..100_000u64 {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(e) = log.append(2, ordinal, &blob(ordinal)) {
                        *stopped.lock().expect("unpoisoned") = Some(e);
                        break;
                    }
                    landed.store(i64::try_from(ordinal).expect("fits"), Ordering::SeqCst);
                    if ordinal == 3 {
                        started.wait();
                        waiting = false;
                    }
                    std::thread::sleep(pause);
                }
                if waiting {
                    started.wait();
                }
            })
        };
        started.wait();
        let fence = log.fence(2, 0, "node-2", 3);
        if fence.is_err() {
            done.store(true, Ordering::Relaxed);
        }
        writer.join().expect("the writer ends");
        let fence = fence.expect("fences");
        assert_eq!(
            *stopped.lock().expect("unpoisoned"),
            Some(LogError::Taken),
            "round {round}: the writer is stopped by the fence"
        );
        assert_eq!(
            landed.load(Ordering::SeqCst),
            i64::try_from(fence).expect("fits").saturating_sub(1),
            "round {round}: every blob the writer landed is below the fence"
        );
    }
}

#[test]
fn deletes_go_by_name() {
    let (store, log) = emulated();
    for ordinal in 0..1205 {
        log.append(4, ordinal, &blob(ordinal)).expect("lands");
    }
    let lists = count(&store, S3Op::List);
    log.delete_range(4, 0, 1200).expect("deletes");
    log.delete_range(4, 10, 10)
        .expect("an empty range deletes nothing");
    assert_eq!(
        count(&store, S3Op::Delete),
        1200,
        "one unbilled DELETE a slot"
    );
    assert_eq!(count(&store, S3Op::List), lists, "and no listing");
    assert_eq!(log.read_entry(4, 1199), Ok(None));
    assert!(log.read_entry(4, 1200).expect("reads").is_some());
}

#[test]
fn a_header_longer_than_the_first_read_takes_a_second() {
    let (store, log) = emulated();
    let keys: Vec<ShardKey> = (0..3000u32)
        .map(|s| key(&format!("uuid-{s}"), s % 100))
        .collect();
    let records: Vec<_> = (0..3000i64)
        .map(|s| [record(&format!("id{s}"), s)])
        .collect();
    let batches: Vec<ShardBatch<'_>> = keys
        .iter()
        .zip(&records)
        .map(|(shard, records)| ShardBatch {
            shard,
            term: 1,
            records,
        })
        .collect();
    let encoded =
        oss_format::node_log_format::encode("node-1", 0, 0, 0, &batches, &[]).expect("encodes");
    assert!(
        encoded.header.body_offset > 64 * 1024,
        "the fixture needs a long header"
    );
    log.append(0, 0, &encoded.bytes).expect("lands");
    let got = header(log.read_entry(0, 0).expect("reads"));
    assert_eq!(count(&store, S3Op::GetRange), 2);
    assert_eq!(got.slices.len(), 3000);
    let last = got.slices[2999].clone();
    assert_eq!(
        ids(&log.read_slice(0, &got, &last).expect("reads")),
        ["id2999"]
    );
}

#[test]
fn a_name_that_cannot_be_a_key_is_refused_up_front() {
    let store: Arc<dyn BlobStore> = Arc::new(oss_blob::Emulator::new(1 << 20, 1));
    assert!(matches!(
        NodeLogStore::new(store, "", "a/b"),
        Err(LogError::Format(_))
    ));
}

#[test]
fn a_blob_shorter_than_its_header_or_slices_is_short() {
    let (_store, log) = emulated();
    // A prefix claiming a 100,000-byte header, and nothing after it.
    let mut cut = b"NLGR\x02".to_vec();
    cut.extend_from_slice(&100_000i32.to_be_bytes());
    cut.resize(70_000, 0);
    log.append(0, 0, &cut).expect("lands");
    assert_eq!(
        log.read_entry(0, 0),
        Err(LogError::Short {
            expected: 100_009,
            actual: 70_000
        })
    );

    let a0 = key("uuid-a", 0);
    let ra = [record("a", 1), record("b", 2)];
    let encoded = window(
        1,
        &[ShardBatch {
            shard: &a0,
            term: 1,
            records: &ra,
        }],
    );
    let body_cut = encoded.bytes.len().saturating_sub(3);
    log.append(3, 1, encoded.bytes.get(..body_cut).expect("in range"))
        .expect("lands");
    let slice = encoded.header.slices[0].clone();
    assert!(matches!(
        log.read_slice(3, &encoded.header, &slice),
        Err(LogError::Short { .. })
    ));
    assert!(matches!(
        log.read_body(3, &encoded.header),
        Err(LogError::Short { .. })
    ));
}

#[test]
fn a_window_without_records_has_an_empty_body_read_without_a_request() {
    let (store, log) = emulated();
    let encoded = window(0, &[]);
    log.append(3, 0, &encoded.bytes).expect("lands");
    let gets = count(&store, S3Op::GetRange);
    assert_eq!(log.read_body(3, &encoded.header), Ok(Vec::new()));
    assert_eq!(count(&store, S3Op::GetRange), gets);
}

#[test]
fn a_read_longer_than_the_bound_is_refused_before_it_is_fetched() {
    let (store, log) = emulated();
    let a0 = key("uuid-a", 0);
    let ra = [record("a", 1), record("b", 2)];
    let encoded = window(
        0,
        &[ShardBatch {
            shard: &a0,
            term: 1,
            records: &ra,
        }],
    );
    log.append(3, 0, &encoded.bytes).expect("lands");
    let slice = encoded.header.slices[0].clone();
    let at_limit = log.clone().with_max_read(slice.length.into());
    assert_eq!(
        ids(&at_limit
            .read_slice(3, &encoded.header, &slice)
            .expect("at the bound")),
        ["a", "b"]
    );
    let below = log
        .clone()
        .with_max_read(u64::from(slice.length).saturating_sub(1));
    let gets = count(&store, S3Op::GetRange);
    assert_eq!(
        below.read_slice(3, &encoded.header, &slice),
        Err(LogError::OutOfRange("read longer than the store's bound"))
    );
    assert!(matches!(
        below.read_body(3, &encoded.header),
        Err(LogError::OutOfRange(_))
    ));
    assert_eq!(
        count(&store, S3Op::GetRange),
        gets,
        "refused before any request"
    );

    // A header longer than the probe and the bound: one probe, no second read.
    let keys: Vec<ShardKey> = (0..3000u32).map(|s| key(&format!("uuid-{s}"), 0)).collect();
    let records: Vec<_> = (0..3000i64)
        .map(|s| [record(&format!("id{s}"), s)])
        .collect();
    let batches: Vec<ShardBatch<'_>> = keys
        .iter()
        .zip(&records)
        .map(|(shard, records)| ShardBatch {
            shard,
            term: 1,
            records,
        })
        .collect();
    let long =
        oss_format::node_log_format::encode("node-1", 0, 0, 0, &batches, &[]).expect("encodes");
    log.append(0, 0, &long.bytes).expect("lands");
    let gets = count(&store, S3Op::GetRange);
    let bounded = log
        .clone()
        .with_max_read(long.header.body_offset.saturating_sub(1));
    assert!(matches!(
        bounded.read_entry(0, 0),
        Err(LogError::OutOfRange(_))
    ));
    assert_eq!(count(&store, S3Op::GetRange), gets.saturating_add(1));
    let exact = log.clone().with_max_read(long.header.body_offset);
    assert_eq!(
        header(exact.read_entry(0, 0).expect("at the bound")),
        long.header
    );
    assert_eq!(
        format!("{:?}", log.with_max_read(u64::MAX)),
        format!(
            "{:?}",
            NodeLogStore::new(store, "", "node-1").expect("valid")
        ),
        "the bound is at most the Java's int"
    );
}

#[test]
fn a_header_within_the_probe_is_one_read() {
    let (store, log) = emulated();
    // About 40 KiB of header: past a probe of a kilobyte, within 64 KiB.
    let keys: Vec<ShardKey> = (0..500u32).map(|s| key(&format!("uuid-{s}"), 0)).collect();
    let records: Vec<_> = (0..500i64)
        .map(|s| [record(&format!("id{s}"), s)])
        .collect();
    let batches: Vec<ShardBatch<'_>> = keys
        .iter()
        .zip(&records)
        .map(|(shard, records)| ShardBatch {
            shard,
            term: 1,
            records,
        })
        .collect();
    let encoded =
        oss_format::node_log_format::encode("node-1", 0, 0, 0, &batches, &[]).expect("encodes");
    assert!(
        (16 * 1024..64 * 1024).contains(&encoded.header.body_offset),
        "{}",
        encoded.header.body_offset
    );
    log.append(0, 0, &encoded.bytes).expect("lands");
    let gets = count(&store, S3Op::GetRange);
    assert_eq!(header(log.read_entry(0, 0).expect("reads")), encoded.header);
    assert_eq!(count(&store, S3Op::GetRange), gets.saturating_add(1));
}
