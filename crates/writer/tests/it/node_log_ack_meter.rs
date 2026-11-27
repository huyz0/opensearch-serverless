//! Where a write acknowledgement's time goes (the shell's
//! `NodeLogCommitTimings`), by criterion of the spec. No upper bound on any
//! time is asserted: only lower bounds the code's own waits guarantee.

use crate::common;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::Hooked;
use common::committer::{commit_all, rec, rig, rig_on, shard, window};
use oss_blob::{BlobStore, Emulator};
use oss_writer::log::{AckTimings, NodeLogStore};

const WINDOW: Duration = Duration::from_millis(50);

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).expect("fits")
}

#[test]
fn one_caller_is_one_blob_one_epoch_and_waits_out_its_window() {
    let r = rig(window(50));
    r.committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands");
    let t = r.committer.timings();
    assert_eq!(
        (t.callers, t.puts, t.epochs_opened, t.admission_checks),
        (1, 1, 1, 1)
    );
    assert!(
        t.window_nanos >= nanos(WINDOW) * 9 / 10,
        "waited for the window: {t:?}"
    );
    assert!(t.put_nanos > 0 && t.write_nanos >= t.put_nanos, "{t:?}");
    assert!(t.window_ms() >= 45.0 && t.put_ms() > 0.0 && t.write_ms() > 0.0);

    r.committer
        .commit(&shard(1), 1, &[rec("b", 2)])
        .expect("lands");
    let t = r.committer.timings();
    assert_eq!(
        (t.callers, t.puts, t.epochs_opened, t.admission_checks),
        (2, 2, 1, 1),
        "the epoch and the shard's admission are not repeated"
    );
}

#[test]
fn thirty_callers_in_one_window_are_one_put() {
    let r = rig(window(50));
    assert!(commit_all(&r, 30).iter().all(Result::is_ok));
    let t = r.committer.timings();
    assert_eq!((t.callers, t.puts, t.admission_checks), (30, 1, 30));
}

#[test]
fn a_caller_behind_a_slow_blob_counts_the_wait_behind_it() {
    let inner = Arc::new(Emulator::new(1 << 20, 3));
    let slow = Arc::new(AtomicBool::new(true));
    let first = Arc::clone(&slow);
    let store = Hooked::store(&inner, move |inner, key, body, buf| {
        if first.swap(false, Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
        inner.create_if_absent(key, body, buf)
    });
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    let r = rig_on(inner, log, window(0));
    let committer = Arc::clone(&r.committer);
    let a = std::thread::spawn(move || committer.commit(&shard(1), 1, &[rec("a", 1)]));
    // Joins while the first blob is in flight.
    std::thread::sleep(Duration::from_millis(50));
    r.committer
        .commit(&shard(2), 1, &[rec("b", 1)])
        .expect("lands");
    a.join().expect("ends").expect("lands");
    let t = r.committer.timings();
    assert_eq!((t.callers, t.puts), (2, 2));
    assert!(
        t.behind_nanos >= nanos(Duration::from_millis(100)),
        "the second waited behind the first: {t:?}"
    );
}

#[test]
fn a_reset_starts_the_sums_over_and_the_line_names_each() {
    let r = rig(window(0));
    r.committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands");
    let line = r.committer.timings().to_string();
    assert!(line.starts_with("callers=1 window="), "{line}");
    assert!(line.contains("| blobs=1 epoch="), "{line}");
    assert!(line.contains("(1 head reads)"), "{line}");
    assert!(
        line.ends_with("| hedged=0 won=0 | epochs opened=1"),
        "{line}"
    );
    r.committer.reset_timings();
    assert_eq!(r.committer.timings(), AckTimings::default());
    assert_eq!(
        AckTimings::default().to_string(),
        "callers=0 window=0.0 behind=0.0 write=0.0 | blobs=0 epoch=0.0 admission=0.0 \
         (0 head reads) put=0.0 ms | hedged=0 won=0 | epochs opened=0"
    );
}

#[test]
fn means_are_sums_over_counts_in_milliseconds() {
    let t = AckTimings {
        callers: 2,
        window_nanos: 3_000_000,
        behind_nanos: 5_000_000,
        write_nanos: 1_000_000,
        puts: 4,
        epoch_nanos: 2_000_000,
        admission_nanos: 6_000_000,
        put_nanos: 10_000_000,
        ..AckTimings::default()
    };
    assert_eq!(
        (t.window_ms(), t.behind_ms(), t.write_ms(), t.put_ms()),
        (1.5, 2.5, 0.5, 2.5)
    );
    let line = t.to_string();
    assert!(
        line.contains("window=1.5 behind=2.5 write=0.5 | blobs=4 epoch=0.5 admission=1.5"),
        "{line}"
    );
    assert!(line.contains("put=2.5 ms"), "{line}");
}
