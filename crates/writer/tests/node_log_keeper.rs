//! The lease as the write log needs it, and the deletes behind it (the
//! shell's `NodeLogWriter.renew` and `deleteBehind`), by criterion of the
//! spec.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::committer::{rec, shard, window};
use common::{count, emulated};
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::NodeLogEpoch;
use oss_format::ShardKey;
use oss_writer::log::{
    CommitError, LeaseKeeper, LeaseWrite, NodeLogCommitter, NodeLogStore, Position, Refusal,
    RenewError, ShardAdmission,
};

/// One lease write: the epochs, whether it opened an epoch, and how many
/// deletes the store had seen when it was written.
type Write = (Vec<NodeLogEpoch>, bool, u64);

/// A lease that records every write, failing while `fail` is set.
struct Lease {
    store: Arc<Emulator>,
    writes: Mutex<Vec<Write>>,
    fail: AtomicBool,
}

impl LeaseWrite for Lease {
    fn write(&self, epochs: &[NodeLogEpoch], opens_epoch: bool) -> Result<(), Refusal> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(Arc::new(std::io::Error::other("lease write failed")));
        }
        self.writes.lock().expect("unpoisoned").push((
            epochs.to_vec(),
            opens_epoch,
            count(&self.store, S3Op::Delete),
        ));
        Ok(())
    }
}

struct Admit;

impl ShardAdmission for Admit {
    fn admit(&self, _: &ShardKey, _: i64, _: u64) -> Result<(), Refusal> {
        Ok(())
    }
}

struct Rig {
    store: Arc<Emulator>,
    log: NodeLogStore,
    lease: Arc<Lease>,
    keeper: Arc<LeaseKeeper>,
    committer: NodeLogCommitter,
}

impl Rig {
    fn writes(&self) -> Vec<Write> {
        self.lease.writes.lock().expect("unpoisoned").clone()
    }

    fn commit(&self, n: u32, id: &str, seq: i64) -> Result<Option<Position>, CommitError> {
        self.committer.commit(&shard(n), 1, &[rec(id, seq)])
    }

    fn exists(&self, epoch: u64, ordinal: u64) -> bool {
        let key = self.log.key(epoch, ordinal).expect("valid");
        self.store.head(&key).expect("heads").is_some()
    }
}

fn epoch(epoch: i64, trim: i64, landed: i64) -> NodeLogEpoch {
    NodeLogEpoch {
        epoch,
        trim,
        landed,
    }
}

fn rig(inherited: Vec<NodeLogEpoch>) -> Rig {
    let (store, log) = emulated();
    let lease = Arc::new(Lease {
        store: Arc::clone(&store),
        writes: Mutex::new(Vec::new()),
        fail: AtomicBool::new(false),
    });
    let keeper = Arc::new(LeaseKeeper::new(
        Box::new(Arc::clone(&lease)),
        log.clone(),
        inherited,
    ));
    let committer = NodeLogCommitter::new(
        log.clone(),
        7,
        Box::new(Arc::clone(&keeper)),
        Box::new(Admit),
        window(0),
    );
    Rig {
        store,
        log,
        lease,
        keeper,
        committer,
    }
}

/// Three slots in epoch 7, then both shards published through slot 1: the
/// trim point is 2.
fn trimmed_to_two(r: &Rig) {
    let a = r.commit(1, "a", 1).expect("lands").expect("some");
    let b = r.commit(2, "b", 1).expect("lands").expect("some");
    r.commit(1, "c", 2).expect("lands");
    r.committer.on_published(&shard(1), a);
    r.committer.on_published(&shard(2), b);
    assert_eq!(r.committer.trim_point(), Some(2));
}

#[test]
fn an_epoch_and_a_renewal_each_write_one_lease_naming_inherited_epochs_first() {
    let inherited = vec![epoch(2, 1, 5)];
    let r = rig(inherited.clone());
    r.commit(1, "a", 1).expect("lands");
    assert_eq!(
        r.writes(),
        [(vec![epoch(2, 1, 5), epoch(7, 0, 0)], true, 0)],
        "the new epoch is recorded once, after the inherited one"
    );
    r.keeper.renew(&r.committer).expect("renews");
    let writes = r.writes();
    assert_eq!(writes.len(), 2);
    assert_eq!(
        writes[1],
        (vec![epoch(2, 1, 5), epoch(7, 0, 1)], false, 0),
        "a renewal names the inherited epochs, then the committer's positions"
    );
}

#[test]
fn a_renewal_deletes_below_the_trim_point_in_one_request_and_then_nothing() {
    let r = rig(Vec::new());
    trimmed_to_two(&r);
    let (deletes, lists) = (count(&r.store, S3Op::Delete), count(&r.store, S3Op::List));
    r.keeper.renew(&r.committer).expect("renews");
    assert_eq!(count(&r.store, S3Op::Delete) - deletes, 1, "one batch");
    assert_eq!(count(&r.store, S3Op::List), lists, "no LIST");
    assert_eq!(
        r.writes().last().map(|w| w.2),
        Some(deletes),
        "the lease landed before the delete"
    );
    assert!(!r.exists(7, 0) && !r.exists(7, 1), "below the trim point");
    assert!(r.exists(7, 2), "shard 1's unpublished slot is kept");

    r.keeper.renew(&r.committer).expect("renews");
    assert_eq!(
        count(&r.store, S3Op::Delete) - deletes,
        1,
        "nothing moved, nothing sent"
    );
}

#[test]
fn a_complete_epoch_is_left_out_of_the_lease_then_deleted_whole() {
    let r = rig(Vec::new());
    trimmed_to_two(&r);
    // A fence ends epoch 7 at slot 3; shard 1 still holds slot 2 in it.
    assert_eq!(r.log.fence(7, 0, "node-2", 3), Ok(3));
    assert!(r.commit(2, "d", 2).is_err());
    let e8 = r.commit(2, "e", 3).expect("lands").expect("some");
    assert_eq!(e8.epoch, 8);
    r.keeper.renew(&r.committer).expect("renews");
    assert!(r.exists(7, 2) && r.exists(7, 3), "epoch 7 is still needed");

    r.committer.on_published(&shard(1), e8);
    r.committer.on_published(&shard(2), e8);
    let deletes = count(&r.store, S3Op::Delete);
    r.keeper.renew(&r.committer).expect("renews");
    let last = r.writes().pop().expect("written");
    assert_eq!(last.0, [epoch(8, 1, 1)], "epoch 7 is left out");
    assert_eq!(last.2, deletes, "before anything of it is deleted");
    for ordinal in 0..8 {
        assert!(!r.exists(7, ordinal), "slot {ordinal} of epoch 7");
    }
    assert!(!r.exists(8, 0), "epoch 8 published through slot 0");
}

#[test]
fn a_failed_lease_write_deletes_nothing_and_a_failed_delete_is_retried() {
    let r = rig(Vec::new());
    trimmed_to_two(&r);
    let deletes = count(&r.store, S3Op::Delete);
    r.lease.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        r.keeper.renew(&r.committer),
        Err(RenewError::Lease(_))
    ));
    assert_eq!(count(&r.store, S3Op::Delete), deletes, "nothing deleted");
    assert!(r.exists(7, 0));

    r.lease.fail.store(false, Ordering::SeqCst);
    r.store.fail_next(S3Op::Delete, 1);
    let renewed = r.keeper.renew(&r.committer);
    assert!(matches!(renewed, Err(RenewError::Delete(_))), "{renewed:?}");
    assert!(r.exists(7, 0), "the failed batch deleted nothing");
    r.keeper.renew(&r.committer).expect("renews");
    assert!(!r.exists(7, 0) && !r.exists(7, 1), "the retry finishes");
    assert!(r.exists(7, 2));
}

#[test]
fn a_complete_epoch_whose_delete_failed_is_deleted_at_the_next_renewal() {
    let r = rig(Vec::new());
    let a = r.commit(1, "a", 1).expect("lands").expect("some");
    assert_eq!(r.log.fence(7, 0, "node-2", 3), Ok(1));
    assert!(r.commit(1, "b", 2).is_err());
    let e8 = r.commit(1, "c", 3).expect("lands").expect("some");
    r.committer.on_published(&shard(1), a);
    r.committer.on_published(&shard(1), e8);
    r.store.fail_next(S3Op::Delete, 1);
    assert!(matches!(
        r.keeper.renew(&r.committer),
        Err(RenewError::Delete(_))
    ));
    assert!(r.exists(7, 1), "the fence is still there");
    r.keeper.renew(&r.committer).expect("renews");
    assert!(!r.exists(7, 0) && !r.exists(7, 1), "deleted on the retry");
    assert_eq!(r.writes().pop().expect("written").0, [epoch(8, 1, 1)]);
}

#[test]
fn inherited_epochs_are_named_in_every_lease_and_never_deleted() {
    let inherited = vec![epoch(2, 0, 3)];
    let r = rig(inherited.clone());
    for ordinal in 0..3 {
        r.log
            .append(2, ordinal, &common::blob(ordinal))
            .expect("appends");
    }
    trimmed_to_two(&r);
    r.keeper.renew(&r.committer).expect("renews");
    // From the trim point the lease names, as a successor fences.
    assert_eq!(r.log.fence(7, 2, "node-2", 3), Ok(3));
    assert!(r.commit(2, "d", 2).is_err());
    let e8 = r.commit(2, "e", 3).expect("lands").expect("some");
    r.committer.on_published(&shard(1), e8);
    r.committer.on_published(&shard(2), e8);
    r.keeper.renew(&r.committer).expect("renews");
    let writes = r.writes();
    assert!(writes.len() >= 4);
    for (epochs, _, _) in &writes {
        assert_eq!(epochs.first(), Some(&inherited[0]), "{epochs:?}");
    }
    for ordinal in 0..3 {
        assert!(r.exists(2, ordinal), "inherited slot {ordinal}");
    }
}

#[test]
fn a_renewal_racing_new_epochs_never_drops_one_a_blob_landed_in() {
    let r = Arc::new(rig(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let renewer = {
        let (r, done) = (Arc::clone(&r), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                r.keeper.renew(&r.committer).expect("renews");
            }
        })
    };
    // Each round lands a blob, then fences its epoch, so the next commit
    // fails and the one after opens a new epoch. Nothing is published, so
    // every epoch stays needed.
    for round in 0..40 {
        let at = r.commit(1, "a", round).expect("lands").expect("some");
        assert_eq!(r.log.fence(at.epoch, 0, "node-2", 3), Ok(at.ordinal + 1));
        assert!(r.commit(1, "b", round).is_err());
    }
    done.store(true, Ordering::SeqCst);
    renewer.join().expect("ends");

    let mut named = std::collections::BTreeSet::new();
    for (epochs, _, _) in r.writes() {
        let now: std::collections::BTreeSet<i64> = epochs.iter().map(|e| e.epoch).collect();
        assert!(named.is_subset(&now), "{named:?} then {now:?}");
        named = now;
    }
    assert_eq!(named.len(), 40, "every epoch is still named");
}
