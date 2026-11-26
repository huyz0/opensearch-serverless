//! docs/specs/open-from-logged-commit: a successor that is slow, is lost, or
//! meets its predecessor again keeps every acknowledged write.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use oss_blob::emulator::hold::Release;
use oss_blob::{BlobStore, S3Op};
use oss_writer::bulk::ItemResult;

use super::writer_open_logged::{
    lost_predecessor, predecessor_checkpointing, register, taken_over_slowly,
};
use super::writer_takeover::{Cluster, Node, cluster, node_logging, takeover, write};

/// The pack deadline of node-2 below: the open margin (60 s) and a second.
const DEADLINE: Duration = Duration::from_millis(61_000);

fn successor(c: &Cluster, one: &Node, term: u64) -> Node {
    let calls = Arc::new(AtomicUsize::new(0));
    node_logging(
        c,
        "node-2",
        term,
        true,
        Some(takeover(c, one, &calls)),
        None,
        (false, true, None),
    )
}

/// The sequence number of the first write a node manages, which is one past
/// what it holds.
fn first_written(node: &Node) -> Option<i64> {
    // The first write after a failure may be refused; a later one is not.
    (0..4).find_map(|_| match write(&node.role, "e") {
        ItemResult::Written { written, .. } => Some(written.seq_no),
        _ => None,
    })
}

// AC-3, AC-6: a replay that takes past the pack deadline is not swapped over
// the adopted commit; the shard opens from the register and loses nothing.
#[test]
fn a_replay_slow_enough_to_cross_the_pack_deadline_opens_from_the_register() {
    let (_, plain) = taken_over_slowly(false, true, Some(DEADLINE), Duration::ZERO);
    let (_, quick) = taken_over_slowly(true, true, Some(DEADLINE), Duration::ZERO);
    assert!(quick.len() > plain.len(), "in time, the commit is adopted");
    let (term, slow) = taken_over_slowly(true, true, Some(DEADLINE), Duration::from_millis(2_000));
    assert_eq!(term, 2);
    assert_eq!(slow, plain, "past the deadline, the register alone");
}

// AC-6: a node lost at each request of its open, whether the request did
// nothing or took effect and was not answered, and a successor of it opens a
// shard that has everything node-1 acknowledged.
#[test]
fn a_successor_lost_at_each_request_of_its_open_loses_nothing() {
    // The writes node-2 makes before it acknowledges one.
    let puts = {
        let c = cluster();
        let one = lost_predecessor(&c, true);
        let two = successor(&c, &one, 2);
        let before = c.store.meter().count(S3Op::Put);
        assert_eq!(first_written(&two), Some(3));
        usize::try_from(c.store.meter().count(S3Op::Put) - before).expect("small")
    };
    assert!(puts >= 3, "an open writes: {puts}");
    for how in [Release::Fail, Release::LandThenFail] {
        for at in 1..=puts {
            let c = cluster();
            let one = lost_predecessor(&c, true);
            let two = successor(&c, &one, 2);
            let holds = c
                .store
                .hold_next(S3Op::Put, u32::try_from(at).expect("small"));
            let lost = std::thread::scope(|s| {
                let writing = s.spawn(|| write(&two.role, "d"));
                for (i, hold) in holds.iter().enumerate() {
                    assert!(hold.wait_arrived(Duration::from_secs(30)), "{how:?} {at}");
                    if i + 1 == at {
                        // Nothing after the lost request gets through.
                        c.store.fail_next(S3Op::Put, 10_000);
                        hold.release(how);
                    } else {
                        hold.release(Release::Land);
                    }
                }
                writing.join().expect("joins")
            });
            c.store.fail_next(S3Op::Put, 0);
            // node-2's own log is not read here, so what it acknowledged is
            // not asserted; what node-1 acknowledged is.
            let _ = lost;
            drop(two);
            let three = successor(&c, &one, 3);
            let next = first_written(&three).expect("a write lands");
            assert!(next >= 3, "a, b and c were kept ({how:?} at {at}, {next})");
        }
    }
}

// AC-7: the predecessor comes back after its successor adopted its commit
// and writes the commit it still had staged: the register is the
// successor's still, and the successor's writes are kept.
#[test]
fn a_predecessor_returning_after_its_successor_adopted_changes_nothing() {
    let c = cluster();
    let one = predecessor_checkpointing(&c, false, Duration::from_millis(400));
    let two = successor(&c, &one, 2);
    assert_eq!(first_written(&two), Some(2));
    let held = register(&c).term;
    assert_eq!(held, 2, "the successor wrote the register");
    // node-1's staged commit is due now and its register write is stale.
    std::thread::sleep(Duration::from_millis(450));
    one.role.publish_due();
    assert_eq!(register(&c).term, 2, "the stale write left the register");
    two.role.publish_due();
    assert_eq!(
        first_written(&two),
        Some(3),
        "the successor carries on with what it held"
    );
}
