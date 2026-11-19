//! A successor reads a shard's records from a fenced log (the reading
//! half of the shell's `NodeLogTakeoverTests`), by criterion of the spec.

mod common;

use common::committer::{Rig, rec, rig, shard, window};
use oss_blob::{BlobStore, S3Op};
use oss_format::{OrdinalRange, UNASSIGNED_SEQ_NO};
use std::time::Duration;

use oss_writer::log::{BodyCache, NodeLogTakeover, Position, TakeoverError};

fn count(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

/// Whole and ranged GETs.
fn gets(r: &Rig) -> u64 {
    count(r, S3Op::Get).saturating_add(count(r, S3Op::GetRange))
}

fn commit(r: &Rig, n: u32, id: &str, seq: i64) -> Position {
    r.committer
        .commit(&shard(n), 1, &[rec(id, seq)])
        .expect("lands")
        .expect("some")
}

fn ids(takeover: &NodeLogTakeover, n: u32) -> Vec<String> {
    let mut out = Vec::new();
    takeover
        .for_each_record(&shard(n), |record| {
            out.push(record.id().to_owned());
            Ok::<_, TakeoverError>(())
        })
        .expect("reads");
    out
}

/// Two epochs: the first fenced under the writer after `a0 b0 a1`, the
/// second holding `a2 b1`; B published its first record.
fn two_epochs(r: &Rig) -> NodeLogTakeover {
    commit(r, 1, "a0", 0);
    let b0 = commit(r, 2, "b0", 0);
    commit(r, 1, "a1", 1);
    assert_eq!(r.log.fence(7, 0, "someone", 1), Ok(3));
    assert!(
        r.committer
            .commit(&shard(1), 1, &[rec("refused", 2)])
            .is_err()
    );
    commit(r, 1, "a2", 2);
    commit(r, 2, "b1", 1);
    r.committer.on_published(&shard(2), b0);
    NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2)
}

#[test]
fn a_successor_reads_every_unpublished_record_of_its_shard_across_epochs() {
    let r = rig(window(0));
    let takeover = two_epochs(&r);
    assert_eq!(ids(&takeover, 1), ["a0", "a1", "a2"]);
    // Epoch 7's last header predates B's publish, so it still names b0;
    // recovery skips a record the restored commit holds.
    assert_eq!(ids(&takeover, 2), ["b0", "b1"]);
    assert!(ids(&takeover, 3).is_empty());
    assert_eq!(takeover.max_seq_no_of(&shard(1)).expect("reads"), 2);
    assert_eq!(
        takeover.max_seq_no_of(&shard(3)).expect("reads"),
        UNASSIGNED_SEQ_NO
    );
    assert!(takeover.holds(&shard(1)).expect("reads"));
    assert!(!takeover.holds(&shard(3)).expect("reads"));
    assert!(
        r.committer
            .commit(&shard(1), 1, &[rec("after", 3)])
            .is_err()
    );
}

#[test]
fn shards_in_one_blob_are_read_with_one_read_of_it() {
    let r = rig(window(0));
    for n in 1..=3 {
        r.committer
            .enqueue(&shard(n), 1, &[rec(&format!("{n}-0"), 0)], |_| {})
            .expect("queued");
    }
    // All three in one blob, as a busy node's window holds many shards.
    r.committer.flush_queued();
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    takeover.fence_all().expect("fences");
    let before = gets(&r);
    assert_eq!(ids(&takeover, 1), ["1-0"]);
    let first = gets(&r) - before;
    assert_eq!(first, 1, "the first shard reads the body once");
    assert_eq!(ids(&takeover, 2), ["2-0"]);
    assert_eq!(ids(&takeover, 3), ["3-0"]);
    assert_eq!(gets(&r) - before, first, "the others read nothing more");
}

#[test]
fn records_after_a_position_and_in_known_runs() {
    let r = rig(window(0));
    let takeover = two_epochs(&r);
    let mut after = Vec::new();
    takeover
        .for_each_record_after(
            &shard(1),
            Position {
                epoch: 7,
                ordinal: 0,
            },
            |record| {
                after.push(record.id().to_owned());
                Ok::<_, TakeoverError>(())
            },
        )
        .expect("reads");
    assert_eq!(after, ["a1", "a2"], "a0's blob and before left out");
    after.clear();
    takeover
        .for_each_record_after(
            &shard(1),
            Position {
                epoch: 8,
                ordinal: 0,
            },
            |record| {
                after.push(record.id().to_owned());
                Ok::<_, TakeoverError>(())
            },
        )
        .expect("reads");
    assert!(after.is_empty(), "every earlier epoch and the blob itself");

    let puts = count(&r, S3Op::Put);
    let own = NodeLogTakeover::new(r.log.clone(), Vec::new(), "node-1", 1);
    let mut known = Vec::new();
    own.for_each_record_in(7, &[OrdinalRange { from: 0, to: 2 }], &shard(1), |record| {
        known.push(record.id().to_owned());
        Ok::<_, TakeoverError>(())
    })
    .expect("reads");
    assert_eq!(known, ["a0", "a1"]);
    assert_eq!(count(&r, S3Op::Put), puts, "no fence");
}

#[derive(Debug, PartialEq)]
enum Stop {
    Enough,
    Takeover,
}

impl From<TakeoverError> for Stop {
    fn from(_: TakeoverError) -> Self {
        Self::Takeover
    }
}

#[test]
fn a_consumer_stops_the_read_and_a_failed_body_read_keeps_nothing() {
    let r = rig(window(0));
    let takeover = two_epochs(&r);
    let mut seen = 0;
    let stopped = takeover.for_each_record(&shard(1), |_| {
        seen += 1;
        if seen == 2 { Err(Stop::Enough) } else { Ok(()) }
    });
    assert_eq!(stopped, Err(Stop::Enough));
    assert_eq!(seen, 2, "nothing after the error");

    let r = rig(window(0));
    for n in 1..=2 {
        r.committer
            .enqueue(&shard(n), 1, &[rec(&format!("{n}-0"), 0)], |_| {})
            .expect("queued");
    }
    r.committer.flush_queued();
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    takeover.fence_all().expect("fences");
    r.store.fail_next(S3Op::GetRange, 1);
    let failed = takeover.for_each_record(&shard(1), |_| Ok::<_, Stop>(()));
    assert_eq!(failed, Err(Stop::Takeover));
    assert_eq!(ids(&takeover, 1), ["1-0"], "read again");
    assert_eq!(ids(&takeover, 2), ["2-0"]);
}

/// `blobs` blobs, each holding one record of shards 1 and 2.
fn shared_blobs(blobs: i64) -> Rig {
    let r = rig(window(0));
    for b in 0..blobs {
        for n in 1..=2 {
            r.committer
                .enqueue(&shard(n), 1, &[rec(&format!("{n}-{b}"), b)], |_| {})
                .expect("queued");
        }
        r.committer.flush_queued();
    }
    r
}

fn takeover_with(r: &Rig, cache: BodyCache) -> NodeLogTakeover {
    let t = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2)
        .with_body_cache(cache);
    t.fence_all().expect("fences");
    t
}

#[test]
fn a_body_is_kept_only_when_shared_and_within_its_bounds() {
    // One shard a blob: read by slice, nothing kept.
    let r = rig(window(0));
    commit(&r, 1, "a0", 0);
    let alone = takeover_with(&r, BodyCache::default());
    assert_eq!(ids(&alone, 1), ["a0"]);
    assert_eq!(alone.kept_body_bytes(), 0, "a blob of one slice");

    let r = shared_blobs(2);
    let t = takeover_with(&r, BodyCache::default());
    assert_eq!(ids(&t, 1), ["1-0", "1-1"]);
    let both = t.kept_body_bytes();
    assert!(
        both > 0 && both.is_multiple_of(2),
        "two bodies of one size: {both}"
    );
    let one = both / 2;

    let exact = BodyCache {
        max_body: one,
        ..BodyCache::default()
    };
    let t = takeover_with(&r, exact);
    assert_eq!(ids(&t, 1), ["1-0", "1-1"]);
    assert_eq!(
        t.kept_body_bytes(),
        both,
        "a body of exactly the bound is kept"
    );

    let under = BodyCache {
        max_body: one - 1,
        ..BodyCache::default()
    };
    let t = takeover_with(&r, under);
    let before = gets(&r);
    assert_eq!(ids(&t, 1), ["1-0", "1-1"]);
    assert_eq!(ids(&t, 2), ["2-0", "2-1"]);
    assert_eq!(t.kept_body_bytes(), 0, "a body over the bound is not");
    assert_eq!(
        gets(&r) - before,
        5,
        "a slice a shard a blob, and the first blob's header once"
    );

    let room_for_one = BodyCache {
        max_kept: one,
        ..BodyCache::default()
    };
    let t = takeover_with(&r, room_for_one);
    assert_eq!(ids(&t, 1), ["1-0", "1-1"]);
    assert_eq!(t.kept_body_bytes(), one, "the second would pass the total");
    let room_for_two = BodyCache {
        max_kept: both,
        ..BodyCache::default()
    };
    let t = takeover_with(&r, room_for_two);
    assert_eq!(ids(&t, 1), ["1-0", "1-1"]);
    assert_eq!(t.kept_body_bytes(), both, "exactly the total fits");
}

#[test]
fn bodies_are_let_go_once_kept_past_their_time() {
    let r = shared_blobs(1);
    let t = takeover_with(
        &r,
        BodyCache {
            keep_for: Duration::ZERO,
            ..BodyCache::default()
        },
    );
    assert_eq!(ids(&t, 1), ["1-0"]);
    std::thread::sleep(Duration::from_millis(2));
    let before = gets(&r);
    assert_eq!(ids(&t, 2), ["2-0"]);
    assert_eq!(gets(&r) - before, 1, "read again after it was let go");
}

fn ids_above(takeover: &NodeLogTakeover, n: u32, through: i64) -> Vec<String> {
    let mut out = Vec::new();
    takeover
        .for_each_record_above(&shard(n), through, |record| {
            out.push(record.id().to_owned());
            Ok::<_, TakeoverError>(())
        })
        .expect("reads");
    out
}

/// Shard 1 in three blobs of its own, a0 a1 a2, and shard 2's b0 between.
fn three_blobs(r: &Rig) -> NodeLogTakeover {
    commit(r, 1, "a0", 0);
    commit(r, 2, "b0", 0);
    commit(r, 1, "a1", 1);
    commit(r, 1, "a2", 2);
    NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2)
}

#[test]
fn only_slices_past_the_commit_are_handed_over() {
    let r = rig(window(0));
    let takeover = three_blobs(&r);
    assert_eq!(ids_above(&takeover, 1, -1), ["a0", "a1", "a2"]);
    assert_eq!(ids_above(&takeover, 1, 0), ["a1", "a2"]);
    assert_eq!(ids_above(&takeover, 1, 1), ["a2"]);
    assert!(ids_above(&takeover, 1, 2).is_empty());
    assert_eq!(ids_above(&takeover, 2, 0), Vec::<String>::new());
}

#[test]
fn slices_the_commit_covers_cost_no_body_or_slice_read() {
    let r = rig(window(0));
    let takeover = three_blobs(&r);
    takeover.fence_all().expect("fences");
    // The headers are what every read pays; the first read keeps them.
    assert_eq!(ids(&takeover, 1), ["a0", "a1", "a2"]);
    let before = gets(&r);
    assert!(ids_above(&takeover, 1, 2).is_empty());
    assert_eq!(gets(&r), before, "nothing covered is fetched");
    let before = gets(&r);
    assert_eq!(ids(&takeover, 1), ["a0", "a1", "a2"]);
    assert_eq!(gets(&r) - before, 3, "a slice read each without the skip");
    let before = gets(&r);
    assert_eq!(ids_above(&takeover, 1, 1), ["a2"]);
    assert_eq!(gets(&r) - before, 1, "only the slice past the commit");
}

#[test]
fn a_slice_with_no_sequence_number_is_never_skipped() {
    let r = rig(window(0));
    r.committer
        .commit(
            &shard(1),
            1,
            &[oss_format::WalRecord::index_unsequenced("u", r#"{"n":1}"#)],
        )
        .expect("lands");
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    assert_eq!(ids_above(&takeover, 1, 1_000_000), ["u"]);
}

#[test]
fn a_slice_mixing_unsequenced_and_sequenced_records_is_judged_by_the_sequenced_ones() {
    let r = rig(window(0));
    r.committer
        .commit(
            &shard(1),
            1,
            &[
                oss_format::WalRecord::index_unsequenced("u", r#"{"n":1}"#),
                rec("s", 5),
            ],
        )
        .expect("lands");
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    assert_eq!(ids_above(&takeover, 1, 4), ["u", "s"], "5 is past 4");
    assert!(
        ids_above(&takeover, 1, 5).is_empty(),
        "covered through 5: the slice is skipped, unsequenced record and all"
    );
}

#[test]
fn a_shared_blob_hands_each_shard_only_its_slices_past_its_commit() {
    let r = rig(window(0));
    r.committer
        .enqueue(&shard(1), 1, &[rec("a", 0)], |_| {})
        .expect("queued");
    r.committer
        .enqueue(&shard(2), 1, &[rec("b", 5)], |_| {})
        .expect("queued");
    r.committer.flush_queued();
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    assert!(ids_above(&takeover, 1, 0).is_empty(), "shard 1 is covered");
    assert_eq!(ids_above(&takeover, 2, 0), ["b"], "shard 2 is not");
    assert_eq!(ids_above(&takeover, 1, -1), ["a"]);
}
