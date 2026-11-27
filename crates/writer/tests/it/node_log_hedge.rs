//! A hedged node-log append: a second identical PUT when the first is slow,
//! landing on whichever answers first, and still telling a fence from its
//! own other PUT (the shell's `NodeLogHedgeTests`).

use crate::common;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::Hooked;
use oss_blob::{BlobError, BlobStore, ByteRange, Emulator, S3Op};
use oss_format::NodeLogEntry;
use oss_writer::log::{HedgeOutcome, LogError, NodeLogStore, Spawn, ThreadPerPut};

/// What the n-th PUT does around the real write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Put {
    /// Writes at once.
    Fast,
    /// Waits, then writes: slow to land.
    SlowBefore,
    /// Writes, then waits before answering: landed, but slow to say so.
    SlowAfter,
    /// Writes, waits, then answers with an error: landed, and the writer
    /// cannot know.
    LandsThenFails,
}

const SLOW: Duration = Duration::from_millis(400);
const HEDGE_AFTER: Duration = Duration::from_millis(50);
const BLOB: &[u8] = b"a window's blob";

/// A store whose PUTs follow a script, one entry per PUT in the order they
/// are sent; `Fast` past its end.
fn scripted_on(inner: &Arc<Emulator>, script: &[Put]) -> NodeLogStore {
    let script = script.to_vec();
    let puts = AtomicUsize::new(0);
    let store = Hooked::store(inner, move |inner, key, body, buf| {
        let n = puts.fetch_add(1, Ordering::SeqCst);
        let what = script.get(n).copied().unwrap_or(Put::Fast);
        if what == Put::SlowBefore {
            std::thread::sleep(SLOW);
        }
        let outcome = inner.create_if_absent(key, body, buf);
        match what {
            Put::SlowAfter => std::thread::sleep(SLOW),
            Put::LandsThenFails => {
                std::thread::sleep(SLOW);
                return Err(BlobError::Unavailable);
            }
            _ => {}
        }
        outcome
    });
    NodeLogStore::new(store, "", "node-1").expect("valid")
}

fn scripted(script: &[Put]) -> (Arc<Emulator>, NodeLogStore) {
    let inner = Arc::new(Emulator::new(1 << 20, 3));
    let log = scripted_on(&inner, script);
    (inner, log)
}

fn blob() -> Arc<[u8]> {
    Arc::from(BLOB)
}

fn stored(log: &NodeLogStore) -> Vec<u8> {
    let mut out = Vec::new();
    log.store()
        .get(
            &log.key(1, 0).expect("key"),
            ByteRange::Full,
            &mut [0u8; 64],
            &mut |c: &[u8]| {
                out.extend_from_slice(c);
                Ok(())
            },
        )
        .expect("stored");
    out
}

fn hedged(log: &NodeLogStore) -> Result<HedgeOutcome, LogError> {
    log.append_hedged(1, 0, &blob(), HEDGE_AFTER, Some(&ThreadPerPut))
}

#[test]
fn a_fast_put_sends_no_hedge() {
    let (inner, log) = scripted(&[]);
    assert_eq!(hedged(&log), Ok(HedgeOutcome::None));
    assert_eq!(inner.meter().count(S3Op::Put), 1);
    assert_eq!(stored(&log), BLOB);
}

#[test]
fn a_slow_put_is_overtaken_by_its_hedge() {
    let (inner, log) = scripted(&[Put::SlowBefore]);
    let started = Instant::now();
    assert_eq!(hedged(&log), Ok(HedgeOutcome::Won));
    assert!(
        started.elapsed() < SLOW,
        "landed by the hedge, not after the slow PUT"
    );
    assert_eq!(stored(&log), BLOB);
    assert_eq!(
        inner.meter().count(S3Op::Get),
        1,
        "only the check above reads"
    );
}

#[test]
fn a_put_that_landed_but_answers_slowly_is_not_mistaken_for_a_fence() {
    let (_inner, log) = scripted(&[Put::SlowAfter]);
    // The hedge finds the slot taken, by the first PUT, which then answers
    // that it landed.
    assert_eq!(hedged(&log), Ok(HedgeOutcome::Sent));
    assert_eq!(stored(&log), BLOB);
}

#[test]
fn a_put_that_landed_and_failed_is_read_back_rather_than_mistaken_for_a_fence() {
    let (inner, log) = scripted(&[Put::LandsThenFails]);
    assert_eq!(hedged(&log), Ok(HedgeOutcome::Sent));
    assert_eq!(
        inner.meter().count(S3Op::Get),
        1,
        "one read back tells this blob from a fence"
    );
}

#[test]
fn a_fenced_slot_fails_both_puts_as_a_fence() {
    let (_inner, log) = scripted(&[]);
    assert_eq!(log.fence(1, 0, "node-2", 7), Ok(0));
    assert_eq!(hedged(&log), Err(LogError::Taken));
    assert!(matches!(
        log.read_entry(1, 0),
        Ok(Some(NodeLogEntry::Fence(_)))
    ));
}

#[test]
fn a_fence_that_wins_the_race_with_both_puts_is_a_fence() {
    // Both PUTs are slow, and a successor's fence lands while they wait.
    let (inner, log) = scripted(&[Put::SlowBefore, Put::SlowBefore]);
    let successor = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        NodeLogStore::new(inner, "", "node-1")
            .expect("valid")
            .fence(1, 0, "node-2", 7)
    });
    assert_eq!(hedged(&log), Err(LogError::Taken));
    assert_eq!(successor.join().expect("ends"), Ok(0));
    assert!(matches!(
        log.read_entry(1, 0),
        Ok(Some(NodeLogEntry::Fence(_)))
    ));
}

#[test]
fn a_slot_taken_by_another_blob_while_the_first_put_is_slow_is_taken() {
    // The first PUT lands late on a slot a fence took; the hedge finds it
    // taken too, and the read-back sees a fence, not this blob.
    let (inner, log) = scripted(&[Put::SlowBefore]);
    let other =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    other.append(1, 0, b"someone else's").expect("lands");
    assert_eq!(hedged(&log), Err(LogError::Taken));
}

#[test]
fn no_hedge_without_a_delay_or_a_spawner() {
    let (inner, log) = scripted(&[Put::SlowBefore]);
    assert_eq!(
        log.append_hedged(1, 0, &blob(), Duration::ZERO, Some(&ThreadPerPut)),
        Ok(HedgeOutcome::None)
    );
    assert_eq!(inner.meter().count(S3Op::Put), 1);
    let (inner, log) = scripted(&[]);
    assert_eq!(
        log.append_hedged(1, 0, &blob(), HEDGE_AFTER, None),
        Ok(HedgeOutcome::None)
    );
    assert_eq!(inner.meter().count(S3Op::Put), 1);
}

/// A spawner that refuses after `allow` jobs.
struct Refusing {
    allow: AtomicUsize,
}

impl Spawn for Refusing {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) -> bool {
        if self
            .allow
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_err()
        {
            return false;
        }
        ThreadPerPut.spawn(job)
    }
}

#[test]
fn a_put_that_cannot_be_sent_is_an_unknown_outcome_and_a_refused_hedge_waits_for_the_first() {
    let (_inner, log) = scripted(&[]);
    let none = Refusing {
        allow: AtomicUsize::new(0),
    };
    assert_eq!(
        log.append_hedged(1, 0, &blob(), HEDGE_AFTER, Some(&none)),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    let (_inner, log) = scripted(&[Put::SlowBefore]);
    let one = Refusing {
        allow: AtomicUsize::new(1),
    };
    assert_eq!(
        log.append_hedged(1, 0, &blob(), HEDGE_AFTER, Some(&one)),
        Ok(HedgeOutcome::Sent)
    );
    let (_inner, log) = scripted(&[Put::LandsThenFails]);
    let one = Refusing {
        allow: AtomicUsize::new(1),
    };
    assert_eq!(
        log.append_hedged(1, 0, &blob(), HEDGE_AFTER, Some(&one)),
        Err(LogError::Blob(BlobError::Unavailable)),
        "a failure with no hedge to tell it from is the store's failure"
    );
}

#[test]
fn both_puts_failing_is_the_stores_failure() {
    let inner = Arc::new(Emulator::new(1 << 20, 3));
    let log = scripted_on(&inner, &[Put::SlowBefore]);
    inner.fail_next(S3Op::Put, 2);
    assert_eq!(hedged(&log), Err(LogError::Blob(BlobError::Unavailable)));
    assert_eq!(log.read_entry(1, 0), Ok(None));
}

/// A spawner that takes every job and runs none: a PUT that never answers.
struct Dropping;

impl Spawn for Dropping {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) -> bool {
        drop(job);
        true
    }
}

#[test]
fn puts_that_never_answer_are_an_unknown_outcome() {
    let (inner, log) = scripted(&[]);
    assert_eq!(
        log.append_hedged(1, 0, &blob(), HEDGE_AFTER, Some(&Dropping)),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    assert_eq!(inner.meter().count(S3Op::Put), 0);
}

#[test]
fn a_read_back_that_fails_or_finds_nothing_is_not_this_blob() {
    // The first PUT lands and fails; the hedge finds the slot taken; the
    // read-back fails, so the outcome is unknown.
    let (inner, log) = scripted(&[Put::LandsThenFails]);
    inner.fail_next(S3Op::Get, 1);
    assert_eq!(hedged(&log), Err(LogError::Blob(BlobError::Unavailable)));

    // Both PUTs say taken, yet the slot is empty when read back.
    let inner = Arc::new(Emulator::new(1 << 20, 3));
    let puts = AtomicUsize::new(0);
    let store = Hooked::store(&inner, move |_, _, _, _| {
        if puts.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(SLOW);
        }
        Ok(oss_blob::CreateOutcome::AlreadyExists)
    });
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    assert_eq!(hedged(&log), Err(LogError::Taken));
    assert_eq!(inner.meter().count(S3Op::Get), 1);
}

#[test]
fn a_slot_holding_a_prefix_of_this_blob_is_not_this_blob() {
    let (inner, log) = scripted(&[Put::SlowBefore]);
    let other =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    other.append(1, 0, &BLOB[..8]).expect("lands");
    assert_eq!(hedged(&log), Err(LogError::Taken));
}

#[test]
fn a_slot_holding_this_blob_and_more_is_not_this_blob() {
    let (inner, log) = scripted(&[Put::SlowBefore]);
    let other =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    other
        .append(1, 0, b"a window's blob, and more")
        .expect("lands");
    assert_eq!(hedged(&log), Err(LogError::Taken));
}
