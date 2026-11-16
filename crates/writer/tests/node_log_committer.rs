//! Group commit across every shard of a node (the shell's
//! `NodeLogCommitterTests`, by criterion of the spec): windows, epochs and admission.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::committer::{commit_all, header_at, puts, rec, rig, shard, window};
use common::ids;
use oss_blob::{BlobStore, S3Op};
use oss_format::{NodeLogEpoch, OrdinalRange, WalRecord};
use oss_writer::log::{CommitError, CommitterConfig, Position};

#[test]
fn a_window_of_any_number_of_shards_is_one_put() {
    for shards in [3u32, 30, 300] {
        let r = rig(window(500));
        let results = commit_all(&r, shards);
        assert_eq!(puts(&r), 1, "{shards} shards in one window: one PUT");
        assert_eq!(r.store.meter().total(), 1, "and no other request");
        let at: BTreeSet<_> = results
            .into_iter()
            .map(|p| p.expect("landed").expect("some"))
            .collect();
        assert_eq!(
            at,
            BTreeSet::from([Position {
                epoch: 7,
                ordinal: 0
            }]),
            "every caller, one position"
        );
        let header = header_at(
            &r,
            Position {
                epoch: 7,
                ordinal: 0,
            },
        );
        assert_eq!(header.slices.len(), usize::try_from(shards).expect("fits"));
        for n in [0, shards - 1] {
            let slice = header.slices_of(&shard(n)).next().expect("a slice").clone();
            assert_eq!(
                ids(&r.log.read_slice(7, &header, &slice).expect("reads")),
                [format!("doc-{n}")]
            );
        }
        assert_eq!(
            (r.committer.blobs(), r.committer.records_committed()),
            (1, u64::from(shards))
        );
        assert_eq!(r.committer.hedges(), (0, 0), "no hedging asked for");
        assert_eq!(r.store.meter().count(S3Op::List), 0);
    }
}

#[test]
fn no_records_write_nothing() {
    let r = rig(window(0));
    assert_eq!(
        r.committer.commit(&shard(0), 1, &[]).expect("nothing"),
        None
    );
    r.committer
        .enqueue(&shard(0), 1, &[], |_| {})
        .expect("nothing");
    r.committer.flush_queued();
    assert_eq!(puts(&r), 0);
    assert!(r.lease.writes.lock().expect("unpoisoned").is_empty());
}

#[test]
fn windows_close_when_full_and_spill_in_order() {
    // Full by records: a window of two records goes without waiting out
    // a long window.
    let r = rig(CommitterConfig {
        window: Duration::from_secs(30),
        max_records: 2,
        ..CommitterConfig::default()
    });
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let c = Arc::clone(&r.committer);
    std::thread::spawn(move || {
        let _ = tx.send(c.commit(&shard(1), 1, &[rec("x", 1), rec("y", 2)]));
    });
    // Waited for at most 5 s, so a window that waits out its 30 s fails here
    // rather than holding the suite.
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Ok(Some(_)))),
        "a full window does not wait out its time"
    );

    // Three records in windows of two: the first window is written full,
    // the third record spills into the next, in order.
    let r = rig(CommitterConfig {
        window: Duration::from_millis(200),
        max_records: 2,
        ..CommitterConfig::default()
    });
    let at = r
        .committer
        .commit(&shard(1), 1, &[rec("a", 1), rec("b", 2), rec("c", 3)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        at,
        Position {
            epoch: 7,
            ordinal: 1
        },
        "the last window's position"
    );
    let first = header_at(
        &r,
        Position {
            epoch: 7,
            ordinal: 0,
        },
    );
    let second = header_at(&r, at);
    let read = |h: &oss_format::NodeLogHeader<'static>| {
        let slice = h.slices[0].clone();
        ids(&r.log.read_slice(7, h, &slice).expect("reads"))
    };
    assert_eq!(read(&first), ["a", "b"]);
    assert_eq!(read(&second), ["c"], "in order, in the next window");
    assert_eq!(
        second.ordinals_of(&shard(1)),
        &[OrdinalRange { from: 0, to: 1 }]
    );

    // Full by bytes: a record larger than what is left of a joined window
    // starts the next one.
    let r = rig(CommitterConfig {
        window: Duration::from_millis(300),
        max_bytes: 100,
        ..CommitterConfig::default()
    });
    let started = Instant::now();
    let small = {
        let c = Arc::clone(&r.committer);
        std::thread::spawn(move || c.commit(&shard(1), 1, &[rec("s", 1)]))
    };
    std::thread::sleep(Duration::from_millis(50));
    let big = WalRecord::index("big", "x".repeat(150), 2, 1, 1);
    let at_big = r
        .committer
        .commit(&shard(2), 1, &[big])
        .expect("lands")
        .expect("some");
    let at_small = small.join().expect("ends").expect("lands").expect("some");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "the window the big one closed is written at once, not after its 300 ms"
    );
    assert_eq!(
        (at_small.ordinal, at_big.ordinal),
        (0, 1),
        "the big one started the next window"
    );
    assert_eq!(puts(&r), 2);
}

#[test]
fn a_fence_ends_the_epoch_and_the_next_window_opens_another() {
    let r = rig(window(0));
    let first = r
        .committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        first,
        Position {
            epoch: 7,
            ordinal: 0
        }
    );
    {
        let writes = r.lease.writes.lock().expect("unpoisoned");
        assert_eq!(
            writes.len(),
            1,
            "the epoch is in the lease before its first blob"
        );
        assert_eq!(
            writes[0],
            [NodeLogEpoch {
                epoch: 7,
                trim: 0,
                landed: 0
            }]
        );
    }
    // A successor fences slot 1.
    assert_eq!(r.log.fence(7, 0, "node-2", 3), Ok(1));
    let fenced = r.committer.commit(&shard(1), 1, &[rec("b", 2)]);
    assert!(
        matches!(
            fenced,
            Err(CommitError::Fenced {
                epoch: 7,
                ordinal: 1
            })
        ),
        "{fenced:?}"
    );
    assert_eq!(r.committer.trim_point(), None, "no epoch is being written");
    let next = r
        .committer
        .commit(&shard(1), 1, &[rec("c", 3)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        next,
        Position {
            epoch: 8,
            ordinal: 0
        },
        "a new epoch"
    );
    let writes = r.lease.writes.lock().expect("unpoisoned");
    assert_eq!(writes.len(), 2);
    assert_eq!(
        writes[1].last(),
        Some(&NodeLogEpoch {
            epoch: 8,
            trim: 0,
            landed: 0
        }),
        "the new epoch is recorded before it is written"
    );
    assert!(
        writes[1].iter().any(|e| e.epoch == 7),
        "the fenced epoch still holds unpublished records"
    );
}

#[test]
fn a_lease_failure_fails_the_window_and_logs_nothing() {
    let r = rig(window(0));
    r.lease.fail.store(true, Ordering::SeqCst);
    let failed = r.committer.commit(&shard(1), 1, &[rec("a", 1)]);
    assert!(matches!(failed, Err(CommitError::Lease(_))), "{failed:?}");
    assert_eq!(puts(&r), 0);
    assert!(
        r.committer.positions().is_empty(),
        "an epoch that was never recorded is not named"
    );
    r.lease.fail.store(false, Ordering::SeqCst);
    let at = r
        .committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        at,
        Position {
            epoch: 7,
            ordinal: 0
        },
        "the epoch is tried again"
    );
}

#[test]
fn an_append_of_unknown_outcome_ends_the_epoch_and_keeps_its_records_unpublished() {
    let r = rig(window(0));
    r.store.fail_next(S3Op::Put, 1);
    let failed = r.committer.commit(&shard(1), 1, &[rec("a", 1)]);
    assert!(matches!(failed, Err(CommitError::Store(_))), "{failed:?}");
    assert_eq!(
        r.committer.unpublished_runs(&shard(1)),
        [(7, vec![OrdinalRange { from: 0, to: 0 }])].into(),
        "it may have landed"
    );
    assert!(
        r.committer.take_complete().is_empty(),
        "not complete while it may hold records"
    );
    let next = r
        .committer
        .commit(&shard(1), 1, &[rec("b", 2)])
        .expect("lands")
        .expect("some");
    assert_eq!(next.epoch, 8);
}

#[test]
fn admission_is_checked_once_an_epoch_and_a_refusal_fails_only_its_shard() {
    let r = rig(window(300));
    r.admission
        .refuse
        .lock()
        .expect("unpoisoned")
        .insert("uuid-1".into());
    let results = commit_all(&r, 3);
    assert!(results[0].as_ref().is_ok_and(Option::is_some));
    assert!(matches!(results[2], Ok(Some(_))));
    assert!(
        matches!(results[1], Err(CommitError::Refused(_))),
        "{:?}",
        results[1]
    );
    assert_eq!(puts(&r), 1, "the rest of the window landed");
    let header = header_at(
        &r,
        Position {
            epoch: 7,
            ordinal: 0,
        },
    );
    assert_eq!(
        header.slices.len(),
        2,
        "the refused shard is not in the blob"
    );

    let checks = || r.admission.checks.lock().expect("unpoisoned").len();
    assert_eq!(checks(), 3);
    r.committer
        .commit(&shard(0), 1, &[rec("again", 9)])
        .expect("lands");
    assert_eq!(checks(), 3, "admitted once an epoch");
    r.committer
        .commit(&shard(0), 2, &[rec("new term", 10)])
        .expect("lands");
    assert_eq!(checks(), 4, "a new term is checked");
    r.committer.forget(&shard(0));
    r.committer
        .commit(&shard(0), 2, &[rec("forgotten", 11)])
        .expect("lands");
    assert_eq!(checks(), 5, "a forgotten shard is checked again");
}

#[test]
fn records_past_a_windows_bytes_go_on_one_window_at_a_time() {
    let r = rig(CommitterConfig {
        window: Duration::from_millis(50),
        max_bytes: 100,
        ..CommitterConfig::default()
    });
    let big = |id: &str| WalRecord::index(id.to_owned(), "x".repeat(59), 1, 1, 1);
    let at = r
        .committer
        .commit(&shard(1), 1, &[big("a"), big("b"), big("c")])
        .expect("lands")
        .expect("some");
    assert_eq!(
        at.ordinal, 2,
        "60 bytes each in windows of 100: one a window"
    );
    assert_eq!(puts(&r), 3);
    for (ordinal, id) in [(0, "a"), (1, "b"), (2, "c")] {
        let h = header_at(&r, Position { epoch: 7, ordinal });
        let slice = h.slices[0].clone();
        assert_eq!(ids(&r.log.read_slice(7, &h, &slice).expect("reads")), [id]);
    }
}

#[test]
fn records_that_exactly_fill_a_window_share_it() {
    let sized = |id: &str, n: usize| WalRecord::index(id.to_owned(), "x".repeat(n), 1, 1, 1);
    // One caller: two records of 60 bytes in a window of 120.
    let r = rig(CommitterConfig {
        window: Duration::from_millis(50),
        max_bytes: 120,
        ..CommitterConfig::default()
    });
    let at = r
        .committer
        .commit(&shard(1), 1, &[sized("a", 59), sized("b", 59)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        (at.ordinal, puts(&r)),
        (0, 1),
        "120 bytes fill it, not overflow it"
    );
    // Two callers: 40 bytes queued, then 60 bytes in a window of 100.
    let r = rig(CommitterConfig {
        window: Duration::from_millis(50),
        max_bytes: 100,
        ..CommitterConfig::default()
    });
    r.committer
        .enqueue(&shard(1), 1, &[sized("q", 39)], |_| {})
        .expect("joins");
    let at = r
        .committer
        .commit(&shard(2), 1, &[sized("c", 59)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        (at.ordinal, puts(&r)),
        (0, 1),
        "the rest of the window is room enough"
    );
}

#[test]
fn a_shards_records_from_two_callers_in_one_window_are_one_slice() {
    let r = rig(window(300));
    let other = {
        let c = Arc::clone(&r.committer);
        std::thread::spawn(move || c.commit(&shard(1), 1, &[rec("first", 1)]))
    };
    std::thread::sleep(Duration::from_millis(50));
    let at = r
        .committer
        .commit(&shard(1), 1, &[rec("second", 2)])
        .expect("lands")
        .expect("some");
    assert_eq!(other.join().expect("ends").expect("lands"), Some(at));
    let h = header_at(&r, at);
    assert_eq!(h.slices.len(), 1);
    let slice = h.slices[0].clone();
    assert_eq!(
        ids(&r.log.read_slice(7, &h, &slice).expect("reads")),
        ["first", "second"]
    );
}
