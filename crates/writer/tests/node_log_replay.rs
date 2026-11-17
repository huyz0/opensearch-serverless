//! A successor reads a shard's records from a fenced log (the reading
//! half of the shell's `NodeLogTakeoverTests`), by criterion of the spec.

mod common;

use common::committer::{Rig, rec, rig, shard, window};
use oss_blob::{BlobStore, S3Op};
use oss_format::{OrdinalRange, UNASSIGNED_SEQ_NO};
use oss_writer::log::{NodeLogTakeover, Position, TakeoverError};

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
