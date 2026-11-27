//! Group commit across every shard of a node (the shell's
//! `NodeLogCommitterTests`, by criterion of the spec): trimming, queued records, idle flush and hedges.

use crate::common;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::Hooked;
use common::committer::{header_at, puts, rec, rig, rig_on, shard, window};
use oss_blob::{BlobStore, Emulator};
use oss_format::{NodeLogEpoch, OrdinalRange};
use oss_writer::log::{CommitError, NodeLogStore, Position, Refusal};

#[test]
fn trimming_follows_publishes_across_epochs() {
    let r = rig(window(0));
    let a = r
        .committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands")
        .expect("some");
    let b = r
        .committer
        .commit(&shard(2), 1, &[rec("b", 1)])
        .expect("lands")
        .expect("some");
    r.committer
        .commit(&shard(1), 1, &[rec("c", 2)])
        .expect("lands");
    assert_eq!(r.committer.trim_point(), Some(0));
    assert_eq!(
        r.committer.last_position(&shard(1)),
        Some(Position {
            epoch: 7,
            ordinal: 2
        })
    );
    r.committer.on_published(&shard(1), a);
    assert_eq!(
        r.committer.trim_point(),
        Some(1),
        "shard 2's slot 1 is now the lowest needed"
    );
    assert_eq!(
        r.committer.unpublished_runs(&shard(1)),
        [(7, vec![OrdinalRange { from: 2, to: 2 }])].into()
    );
    r.committer.on_published(&shard(2), b);
    assert_eq!(r.committer.trim_point(), Some(2));
    assert_eq!(
        r.committer.positions(),
        [NodeLogEpoch {
            epoch: 7,
            trim: 2,
            landed: 3
        }]
    );

    // A fence ends epoch 7; shard 1 still holds slot 2 in it.
    assert_eq!(r.log.fence(7, 0, "node-2", 3), Ok(3));
    assert!(r.committer.commit(&shard(2), 1, &[rec("d", 2)]).is_err());
    let e8 = r
        .committer
        .commit(&shard(2), 1, &[rec("e", 3)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        r.committer.positions().len(),
        2,
        "epoch 7 still holds shard 1's slot 2"
    );
    assert!(r.committer.take_complete().is_empty());
    // A publish in a later epoch covers everything in earlier ones.
    r.committer.on_published(&shard(1), e8);
    r.committer.on_published(&shard(2), e8);
    assert_eq!(r.committer.last_position(&shard(1)), None);
    assert_eq!(
        r.committer.take_complete(),
        [NodeLogEpoch {
            epoch: 7,
            trim: 3,
            landed: 3
        }]
    );
    assert!(r.committer.take_complete().is_empty(), "handed back once");
    assert_eq!(
        r.committer.positions(),
        [NodeLogEpoch {
            epoch: 8,
            trim: 1,
            landed: 1
        }]
    );
}

#[test]
fn enqueued_records_ride_the_next_blob() {
    let r = rig(window(100));
    let told = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&told);
    r.committer
        .enqueue(&shard(1), 1, &[rec("queued", 1)], move |at| {
            *sink.lock().expect("unpoisoned") = Some(at);
        })
        .expect("joins");
    assert_eq!(puts(&r), 0, "nothing of its own");
    let at = r
        .committer
        .commit(&shard(2), 1, &[rec("carrier", 1)])
        .expect("lands")
        .expect("some");
    assert_eq!(puts(&r), 1, "rode the carrier's blob");
    assert_eq!(*told.lock().expect("unpoisoned"), Some(at));
    assert_eq!(header_at(&r, at).slices.len(), 2);

    r.committer.flush_queued();
    assert_eq!(puts(&r), 1, "nothing queued, nothing written");
    let told = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&told);
    r.committer
        .enqueue(&shard(1), 1, &[rec("alone", 2)], move |at| {
            *sink.lock().expect("unpoisoned") = Some(at);
        })
        .expect("joins");
    r.committer.flush_queued();
    assert_eq!(puts(&r), 2, "one write for what nothing else carried");
    assert_eq!(
        *told.lock().expect("unpoisoned"),
        Some(Position {
            epoch: 7,
            ordinal: 1
        })
    );
}

#[test]
fn an_enqueued_record_that_does_not_land_is_not_told() {
    let r = rig(window(0));
    r.admission
        .refuse
        .lock()
        .expect("unpoisoned")
        .insert("uuid-1".into());
    let told = Arc::new(AtomicBool::new(false));
    let sink = Arc::clone(&told);
    r.committer
        .enqueue(&shard(1), 1, &[rec("refused", 1)], move |_| {
            sink.store(true, Ordering::SeqCst)
        })
        .expect("joins");
    r.committer
        .commit(&shard(2), 1, &[rec("carrier", 1)])
        .expect("lands");
    assert!(!told.load(Ordering::SeqCst));
}

#[test]
fn idle_flush_writes_a_lone_record_at_once() {
    let r = rig(window(250));
    let started = Instant::now();
    r.committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands");
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "without it, a lone record waits its window"
    );
    r.committer.flush_when_idle(true);
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    r.committer
        .commit(&shard(1), 1, &[rec("b", 2)])
        .expect("lands");
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "the log was quiet for a window: at once"
    );
}

#[test]
fn a_hedged_window_counts_its_hedges() {
    // Every PUT is slow; a hedge after 20 ms wins against the first.
    let inner = Arc::new(Emulator::new(1 << 30, 9));
    let puts = std::sync::atomic::AtomicUsize::new(0);
    let store = Hooked::store(&inner, move |inner, key, body, buf| {
        if puts.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(300));
        }
        inner.create_if_absent(key, body, buf)
    });
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    let r = rig_on(Arc::clone(&inner), log, window(0));
    r.committer.hedge_appends(
        Duration::from_millis(20),
        Some(Arc::new(oss_writer::log::ThreadPerPut)),
    );
    assert_eq!(r.committer.hedge_delay(100), Duration::from_millis(20));
    r.committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands");
    assert_eq!(r.committer.hedges(), (1, 1));
    r.committer.hedge_appends(Duration::ZERO, None);
    r.committer
        .commit(&shard(1), 1, &[rec("b", 2)])
        .expect("lands");
    assert_eq!(r.committer.hedges(), (1, 1), "no hedge, none counted");
    assert_eq!(r.committer.blobs(), 2);
    assert_eq!(r.committer.hedge_delay(100), Duration::ZERO);
}

#[test]
fn errors_name_what_failed() {
    let lease: Refusal = Arc::new(std::io::Error::other("no lease"));
    let cases = [
        (
            CommitError::Fenced {
                epoch: 3,
                ordinal: 4,
            },
            "epoch 3 of the node log was fenced at slot 4",
        ),
        (
            CommitError::Store(oss_writer::log::LogError::Taken),
            "append failed",
        ),
        (CommitError::Lease(Arc::clone(&lease)), "no lease"),
        (CommitError::Refused(lease), "refused to log"),
        (
            CommitError::Encode(oss_format::FormatError::Corrupt {
                what: "w",
                reason: "r",
            }),
            "could not encode",
        ),
        (CommitError::Lost, "without an outcome"),
    ];
    for (error, part) in cases {
        assert!(error.to_string().contains(part), "{error} names {part:?}");
    }
    let r = rig(window(0));
    assert!(format!("{:?}", r.committer).contains("NodeLogCommitter"));
    let _: &dyn BlobStore = r.store.as_ref();
}

#[test]
fn a_blob_that_cannot_be_encoded_fails_its_window_and_keeps_the_epoch() {
    let r = rig(window(0));
    let huge = oss_format::ShardKey::new("u".repeat(70_000), 0, None::<&str>);
    let failed = r.committer.commit(&huge, 1, &[rec("a", 1)]);
    assert!(matches!(failed, Err(CommitError::Encode(_))), "{failed:?}");
    assert_eq!(puts(&r), 0);
    let at = r
        .committer
        .commit(&shard(1), 1, &[rec("b", 1)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        at,
        Position {
            epoch: 7,
            ordinal: 0
        },
        "nothing was written, so the epoch goes on"
    );
}

/// A lease whose write panics once.
struct Panicking(AtomicBool);

impl oss_writer::log::EpochLease for Panicking {
    fn record(&self, _: &[NodeLogEpoch]) -> Result<(), Refusal> {
        if !self.0.swap(true, Ordering::SeqCst) {
            panic!("the lease write panicked");
        }
        Ok(())
    }
}

#[test]
fn a_writer_that_panics_fails_its_window_and_leaves_the_committer_usable() {
    let (store, log) = common::emulated();
    let committer = Arc::new(oss_writer::log::NodeLogCommitter::new(
        log,
        0,
        Box::new(Panicking(AtomicBool::new(false))),
        Box::new(Arc::new(common::committer::Admission::default())),
        window(200),
    ));
    let waiter = {
        let c = Arc::clone(&committer);
        std::thread::spawn(move || c.commit(&shard(2), 1, &[rec("waits", 1)]))
    };
    std::thread::sleep(Duration::from_millis(50));
    let driver = {
        let c = Arc::clone(&committer);
        std::thread::spawn(move || c.commit(&shard(1), 1, &[rec("drives", 1)]))
    };
    let (waited, drove) = (waiter.join(), driver.join());
    // Whichever thread wrote the window panicked; the other was told Lost.
    let outcomes = [waited, drove];
    assert_eq!(
        outcomes.iter().filter(|o| o.is_err()).count(),
        1,
        "the writer's panic goes on"
    );
    assert!(
        outcomes
            .iter()
            .any(|o| matches!(o, Ok(Err(CommitError::Lost))))
    );
    assert_eq!(store.meter().count(oss_blob::S3Op::Put), 0);
    assert!(
        committer.positions().is_empty(),
        "the epoch it was opening is not named"
    );
    let at = committer
        .commit(&shard(1), 1, &[rec("after", 2)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        at,
        Position {
            epoch: 0,
            ordinal: 0
        }
    );
}

#[test]
fn a_put_that_panics_after_landing_ends_the_epoch_and_keeps_its_records_unpublished() {
    let inner = Arc::new(Emulator::new(1 << 30, 9));
    let panicked = AtomicBool::new(false);
    let store = Hooked::store(&inner, move |inner, key, body, buf| {
        let outcome = inner.create_if_absent(key, body, buf);
        if !panicked.swap(true, Ordering::SeqCst) {
            panic!("the PUT landed, then its thread panicked");
        }
        outcome
    });
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    let r = rig_on(Arc::clone(&inner), log, window(0));
    let c = Arc::clone(&r.committer);
    let wrote = std::thread::spawn(move || c.commit(&shard(1), 1, &[rec("a", 1)])).join();
    assert!(wrote.is_err(), "the panic goes on");
    assert_eq!(
        r.committer.unpublished_runs(&shard(1)),
        [(7, vec![OrdinalRange { from: 0, to: 0 }])].into(),
        "it may have landed"
    );
    assert_eq!(r.committer.blobs(), 0, "not counted as landed");
    let next = r
        .committer
        .commit(&shard(1), 1, &[rec("b", 2)])
        .expect("lands")
        .expect("some");
    assert_eq!(
        next,
        Position {
            epoch: 8,
            ordinal: 0
        },
        "a new epoch, not the slot it may have taken"
    );
}
