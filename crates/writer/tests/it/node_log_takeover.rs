//! A successor fences a predecessor's write log (the fencing half of the
//! shell's `NodeLogTakeoverTests`), by criterion of the spec.

use crate::common;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::committer::{Rig, rec, rig, shard, window};
use oss_blob::{BlobStore, S3Op};
use oss_format::NodeLogEpoch;
use oss_writer::log::{CommitError, LeaseReader, NodeLogTakeover, Refusal, TakeoverError};

fn count(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

fn commit(r: &Rig, n: u32, id: &str, seq: i64) -> Result<(), CommitError> {
    r.committer
        .commit(&shard(n), 1, &[rec(id, seq)])
        .map(|_| ())
}

fn reading(epochs: Vec<NodeLogEpoch>) -> Box<dyn LeaseReader> {
    Box::new(move || -> Result<Vec<NodeLogEpoch>, Refusal> { Ok(epochs.clone()) })
}

#[test]
fn a_fenced_writer_lands_nothing_more() {
    let r = rig(window(0));
    commit(&r, 1, "a", 1).expect("lands");
    commit(&r, 2, "b", 1).expect("lands");
    let takeover = NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    assert_eq!(takeover.ends().expect("fences"), BTreeMap::from([(7, 2)]));
    assert_eq!(takeover.predecessor(), "node-1");
    assert_eq!(takeover.epochs(), r.committer.positions());
    assert!(matches!(
        commit(&r, 1, "after", 2),
        Err(CommitError::Fenced {
            epoch: 7,
            ordinal: 2
        })
    ));
}

#[test]
fn shards_taken_from_one_predecessor_share_its_fences() {
    let r = rig(window(0));
    for w in 0..4 {
        for n in 1..=3 {
            // A zero window: one blob a commit, each shard in four of its own.
            commit(&r, n, &format!("{n}-{w}"), w).expect("lands");
        }
    }
    let lease = r.committer.positions();
    let (puts, lists) = (count(&r, S3Op::Put), count(&r, S3Op::List));
    let takeover = NodeLogTakeover::new(r.log.clone(), lease.clone(), "node-2", 2);
    takeover.fence_all().expect("fences");
    assert_eq!(takeover.ends().expect("fenced"), BTreeMap::from([(7, 12)]));
    assert_eq!(
        takeover
            .named()
            .expect("fenced")
            .into_iter()
            .collect::<Vec<_>>(),
        [shard(1), shard(2), shard(3)]
    );
    assert_eq!(count(&r, S3Op::Put) - puts, 1, "one fence for every shard");
    assert_eq!(count(&r, S3Op::List), lists, "and no listing");

    let puts = count(&r, S3Op::Put);
    let second = NodeLogTakeover::new(r.log.clone(), lease, "node-3", 2);
    assert_eq!(second.ends().expect("fences"), BTreeMap::from([(7, 12)]));
    assert_eq!(count(&r, S3Op::Put), puts, "a second successor shares it");
}

/// Two blobs, a lease read with a landed hint of 2, four more blobs, then
/// a publish, its lease and a delete behind it, then one more blob: the
/// stale lease and the current one.
fn deleted_behind_a_stale_lease(r: &Rig) -> (Vec<NodeLogEpoch>, Vec<NodeLogEpoch>) {
    commit(r, 1, "old-0", 0).expect("lands");
    commit(r, 1, "old-1", 1).expect("lands");
    let stale = r.committer.positions();
    for i in 2..6 {
        commit(r, 1, &format!("old-{i}"), i).expect("lands");
    }
    let last = r.committer.last_position(&shard(1)).expect("landed");
    r.committer.on_published(&shard(1), last);
    let trim = r.committer.trim_point().expect("writing");
    assert_eq!(trim, 6);
    r.log.delete_range(7, 0, trim).expect("deletes");
    commit(r, 2, "b-after", 0).expect("lands");
    assert_eq!(stale[0].landed, 2, "the fence alone would land in the hole");
    (stale, r.committer.positions())
}

#[test]
fn a_fence_from_a_stale_lease_is_searched_for_again_above_the_deleted_slots() {
    let r = rig(window(0));
    let (stale, now) = deleted_behind_a_stale_lease(&r);
    let takeover = NodeLogTakeover::rereading(r.log.clone(), stale, "node-2", 2, reading(now));
    assert_eq!(takeover.ends().expect("fences"), BTreeMap::from([(7, 7)]));
    assert_eq!(
        takeover
            .named()
            .expect("fenced")
            .into_iter()
            .collect::<Vec<_>>(),
        [shard(2)],
        "the header search starts at the current trim point"
    );
    assert!(commit(&r, 2, "after-the-fence", 1).is_err());
}

#[test]
fn without_the_reread_a_stale_fence_lands_in_the_hole() {
    let r = rig(window(0));
    let (stale, _) = deleted_behind_a_stale_lease(&r);
    let takeover = NodeLogTakeover::new(r.log.clone(), stale, "node-2", 2);
    assert_eq!(
        takeover.ends().expect("fences"),
        BTreeMap::from([(7, 2)]),
        "in the hole, below the writer's slot 7"
    );
    commit(&r, 2, "still-writing", 1).expect("the writer never reaches the hole");
}

#[test]
fn an_epoch_with_nothing_above_its_trim_point_names_no_shard() {
    let r = rig(window(0));
    commit(&r, 1, "a0", 0).expect("lands");
    let at = r.committer.last_position(&shard(1)).expect("landed");
    r.committer.on_published(&shard(1), at);
    let lease = r.committer.positions();
    assert_eq!(
        lease,
        [NodeLogEpoch {
            epoch: 7,
            trim: 1,
            landed: 1
        }]
    );
    let gets = |r: &Rig| count(r, S3Op::Get) + count(r, S3Op::GetRange);
    let (puts, got) = (count(&r, S3Op::Put), gets(&r));
    let takeover = NodeLogTakeover::new(r.log.clone(), lease, "node-2", 2);
    assert!(takeover.named().expect("fences").is_empty());
    assert_eq!(count(&r, S3Op::Put) - puts, 1, "the fence");
    assert_eq!(
        gets(&r) - got,
        1,
        "the fence's look at the slot below it for another successor's fence, and no header read"
    );
}

#[test]
fn an_unfenced_reader_writes_nothing_and_the_writer_goes_on() {
    let r = rig(window(0));
    commit(&r, 1, "a", 1).expect("lands");
    commit(&r, 2, "b", 1).expect("lands");
    let lease = r.committer.positions();
    let puts = count(&r, S3Op::Put);
    let reader = NodeLogTakeover::unfenced(r.log.clone(), lease.clone(), reading(lease));
    assert_eq!(reader.ends().expect("reads"), BTreeMap::from([(7, 2)]));
    assert_eq!(reader.predecessor(), "node-1");
    assert_eq!(reader.named().expect("read").len(), 2);
    assert_eq!(count(&r, S3Op::Put), puts, "nothing written");
    commit(&r, 1, "after", 2).expect("the writer goes on");
}

#[test]
fn a_failed_fence_or_reread_caches_nothing_and_the_next_call_succeeds() {
    let r = rig(window(0));
    commit(&r, 1, "a", 1).expect("lands");
    let lease = r.committer.positions();
    r.store.fail_next(S3Op::Put, 1);
    let takeover = NodeLogTakeover::new(r.log.clone(), lease.clone(), "node-2", 2);
    assert!(matches!(takeover.fence_all(), Err(TakeoverError::Log(_))));
    assert_eq!(takeover.ends().expect("fences"), BTreeMap::from([(7, 1)]));

    let fail = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&fail);
    let now = lease.clone();
    let reread = Box::new(move || -> Result<Vec<NodeLogEpoch>, Refusal> {
        if flag.swap(false, Ordering::SeqCst) {
            return Err(Arc::new(std::io::Error::other("lease unreadable")));
        }
        Ok(now.clone())
    });
    let second = NodeLogTakeover::rereading(r.log.clone(), lease, "node-3", 2, reread);
    let failed = second.fence_all();
    assert!(matches!(failed, Err(TakeoverError::Lease(_))), "{failed:?}");
    assert_eq!(
        second.ends().expect("fences"),
        BTreeMap::from([(7, 1)]),
        "shared"
    );
}

#[test]
fn a_takeover_failure_says_what_failed() {
    let lease = TakeoverError::Lease(Arc::new(std::io::Error::other("gone")));
    assert_eq!(lease.to_string(), "the predecessor's lease: gone");
    let log = TakeoverError::from(oss_writer::log::LogError::Taken);
    assert!(
        log.to_string().starts_with("the predecessor's log: "),
        "{log}"
    );
}

#[test]
fn a_reread_finding_the_fence_safe_reads_the_lease_once_and_fences_once() {
    let r = rig(window(0));
    commit(&r, 1, "a0", 0).expect("lands");
    let at = r.committer.last_position(&shard(1)).expect("landed");
    r.committer.on_published(&shard(1), at);
    let lease = r.committer.positions();
    let requests = |r: &Rig| {
        count(r, S3Op::Head) + count(r, S3Op::Get) + count(r, S3Op::GetRange) + count(r, S3Op::Put)
    };

    let before = requests(&r);
    NodeLogTakeover::new(r.log.clone(), lease.clone(), "node-2", 2)
        .fence_all()
        .expect("fences");
    let plain = requests(&r) - before;

    // The trim point equals the fence's slot: nothing was deleted past it.
    let reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = Arc::clone(&reads);
    let now = lease.clone();
    let reread = Box::new(move || -> Result<Vec<NodeLogEpoch>, Refusal> {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(now.clone())
    });
    let before = requests(&r);
    let second = NodeLogTakeover::rereading(r.log.clone(), lease, "node-3", 2, reread);
    assert_eq!(second.ends().expect("fences"), BTreeMap::from([(7, 1)]));
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the lease is read once");
    assert_eq!(requests(&r) - before, plain, "and the epoch fenced once");
}

#[test]
fn a_fence_moved_once_reads_the_lease_twice() {
    let r = rig(window(0));
    let (stale, now) = deleted_behind_a_stale_lease(&r);
    let reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = Arc::clone(&reads);
    let reread = Box::new(move || -> Result<Vec<NodeLogEpoch>, Refusal> {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(now.clone())
    });
    let takeover = NodeLogTakeover::rereading(r.log.clone(), stale, "node-2", 2, reread);
    assert_eq!(takeover.ends().expect("fences"), BTreeMap::from([(7, 7)]));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        2,
        "once to find the hole, once to see the new fence is safe"
    );
}
