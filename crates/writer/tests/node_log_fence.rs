//! Fences against a writer that wins the race for the slot the fence aims
//! at, placed deterministically by the store, and the log's failures.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use common::{Hooked, blob};
use oss_blob::{BlobError, BlobStore, Emulator, S3Op, SliceSource};
use oss_format::NodeLogEntry;
use oss_format::node_log_format::FENCE_MAGIC;
use oss_writer::log::{FENCE_ATTEMPTS, LogError, NodeLogStore};

/// How the writer answers each fence PUT it beats.
#[derive(Clone, Copy)]
enum Writer {
    /// Appends every slot from its next up to the fence's target, as a
    /// writer appending back to back does.
    Burst,
    /// Takes the target alone: no writer does, but it keeps a fence losing.
    TakesTarget,
    /// As `Burst`, and once the fence lands a barrier above it, takes every
    /// slot up to the barrier too.
    BurstToBarrier,
}

/// A log whose writer beats the first `wins` fence PUTs.
fn racing(wins: u32, writer: Writer) -> (Arc<Emulator>, NodeLogStore, Arc<Mutex<u64>>) {
    let inner = Arc::new(Emulator::new(1 << 30, 5));
    let wins = AtomicU32::new(wins);
    let next = Arc::new(Mutex::new(0u64));
    let landed = Arc::clone(&next);
    let store = Hooked::store(&inner, move |inner, key, body, buf| {
        let n = body.fill(buf)?;
        body.rewind()?;
        let fence = buf.get(..4.min(n)) == Some(&FENCE_MAGIC.to_be_bytes()[..]);
        if fence
            && wins
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |w| w.checked_sub(1))
                .is_ok()
        {
            let (dir, slot) = key.split_at(key.len().saturating_sub(20));
            let target: u64 = slot.parse().expect("a slot");
            let mut next = landed.lock().expect("unpoisoned");
            let from = match writer {
                Writer::Burst | Writer::BurstToBarrier => *next,
                Writer::TakesTarget => target,
            };
            for ordinal in from..=target {
                let bytes = blob(ordinal);
                inner.create_if_absent(
                    &format!("{dir}{ordinal:020}"),
                    &mut SliceSource::new(&bytes),
                    buf,
                )?;
            }
            *next = target.saturating_add(1);
        }
        let outcome = inner.create_if_absent(key, body, buf)?;
        if fence && matches!(writer, Writer::BurstToBarrier) {
            let (dir, slot) = key.split_at(key.len().saturating_sub(20));
            let target: u64 = slot.parse().expect("a slot");
            let mut next = landed.lock().expect("unpoisoned");
            for ordinal in *next..target {
                let bytes = blob(ordinal);
                inner.create_if_absent(
                    &format!("{dir}{ordinal:020}"),
                    &mut SliceSource::new(&bytes),
                    buf,
                )?;
            }
            *next = target.max(*next);
        }
        Ok(outcome)
    });
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    (inner, log, next)
}

#[test]
fn a_fence_that_loses_twice_aims_ahead_and_fills_the_gap_from_the_bottom() {
    let (_inner, log, landed) = racing(2, Writer::Burst);
    // The writer takes 0 ahead of the first try, then 1 and 2 ahead of a
    // fence aimed at 2; the fence aims at 5, lands, and fills 3.
    let fence = log.fence(0, 0, "node-2", 4).expect("fences");
    assert_eq!(fence, 3, "where the writer stopped");
    assert_eq!(
        _inner.meter().count(S3Op::GetRange),
        4,
        "a GET after each lost PUT and of each slot below a try; none after a fence lands"
    );
    assert_eq!(
        *landed.lock().expect("unpoisoned"),
        fence,
        "every blob it landed is below"
    );
    assert_eq!(log.append(0, 3, &blob(3)), Err(LogError::Taken));
    assert!(
        matches!(log.read_entry(0, 5), Ok(Some(NodeLogEntry::Fence(_)))),
        "the barrier stays"
    );
    assert_eq!(log.read_entry(0, 4), Ok(None));
}

#[test]
fn a_fence_that_loses_once_lands_below_its_barrier() {
    let (_inner, log, landed) = racing(1, Writer::Burst);
    assert_eq!(log.fence(0, 0, "node-2", 4), Ok(1));
    assert_eq!(*landed.lock().expect("unpoisoned"), 1);
}

#[test]
fn a_writer_that_reaches_the_barrier_stops_there() {
    // The writer takes 0, then 1 and 2, then 3 to 5 ahead of the fence's
    // tries; the fence lands its barrier at 10 and fills 6.
    let (_inner, log, landed) = racing(3, Writer::Burst);
    let fence = log.fence(0, 0, "node-2", 4).expect("fences");
    assert_eq!(fence, 6);
    assert!(
        matches!(log.read_entry(0, 10), Ok(Some(NodeLogEntry::Fence(_)))),
        "the gap doubled: the barrier is at 6 + 4"
    );
    assert_eq!(*landed.lock().expect("unpoisoned"), fence);
    for slot in 0..fence {
        assert!(matches!(
            log.read_entry(0, slot),
            Ok(Some(NodeLogEntry::Records(_)))
        ));
    }
}

#[test]
fn a_writer_that_keeps_taking_the_target_exhausts_the_fence() {
    let (inner, log, _) = racing(FENCE_ATTEMPTS, Writer::TakesTarget);
    assert_eq!(log.fence(0, 0, "node-2", 4), Err(LogError::FenceExhausted));
    assert_eq!(inner.meter().count(S3Op::List), 0);
}

#[test]
fn a_store_failure_stops_a_fence_and_an_append() {
    let inner = Arc::new(Emulator::new(1 << 20, 5));
    let log =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    inner.fail_next(S3Op::Put, 1);
    assert_eq!(
        log.fence(0, 0, "node-2", 4),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    inner.fail_next(S3Op::Put, 1);
    assert_eq!(
        log.append(0, 0, &blob(0)),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    inner.fail_next(S3Op::Head, 1);
    assert_eq!(
        log.first_free(0, 0),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    inner.fail_next(S3Op::Delete, 1);
    assert_eq!(
        log.delete_range(0, 0, 1),
        Err(LogError::Blob(BlobError::Unavailable))
    );
    assert_eq!(
        log.fence(0, 0, &"x".repeat(70_000), 4),
        Err(LogError::Format(oss_format::FormatError::Corrupt {
            what: "node-log blob",
            reason: "a string past 65,535 bytes of modified UTF-8",
        }))
    );
}

#[test]
fn errors_say_what_failed_and_the_store_says_whose_log_it_is() {
    let inner: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 20, 5));
    let log = NodeLogStore::new(inner, "base/", "node-1").expect("valid");
    assert_eq!(log.node_id(), "node-1");
    assert!(format!("{log:?}").contains("node-1"));
    assert_eq!(
        log.key(2, 3).expect("key"),
        format!("base/nodelog/node-1/e={:020}/{:020}", 2, 3)
    );
    let messages = [
        LogError::Taken,
        LogError::Blob(BlobError::NotFound),
        LogError::Format(oss_format::FormatError::Corrupt {
            what: "w",
            reason: "r",
        }),
        LogError::Short {
            expected: 5,
            actual: 3,
        },
        LogError::FenceExhausted,
        LogError::OutOfRange("slot"),
    ]
    .map(|e| e.to_string());
    for (message, part) in messages.iter().zip([
        "taken",
        "request failed",
        "malformed w: r",
        "5 bytes came back as 3",
        "kept taking",
        "slot out of range",
    ]) {
        assert!(message.contains(part), "{message:?} names {part:?}");
    }
}

#[test]
fn a_writer_that_fills_the_gap_is_stopped_at_the_barrier() {
    // The writer takes 0, then 1 and 2; the barrier lands at 5, and the
    // writer takes 3 and 4 before the fence fills: every gap slot is the
    // writer's, so the barrier is where it stopped.
    let (_inner, log, landed) = racing(2, Writer::BurstToBarrier);
    assert_eq!(log.fence(0, 0, "node-2", 4), Ok(5));
    assert_eq!(*landed.lock().expect("unpoisoned"), 5);
    for slot in 0..5 {
        assert!(
            matches!(log.read_entry(0, slot), Ok(Some(NodeLogEntry::Records(_)))),
            "slot {slot}"
        );
    }
    assert!(matches!(
        log.read_entry(0, 5),
        Ok(Some(NodeLogEntry::Fence(_)))
    ));
    let below = log
        .last_header_below(0, 5, 0)
        .expect("reads")
        .expect("a header");
    assert_eq!(below.ordinal, 4);
}

#[test]
fn a_fence_another_successor_put_below_the_first_free_slot_is_shared() {
    let inner = Arc::new(Emulator::new(1 << 20, 5));
    let other =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    for slot in 0..3 {
        other.append(0, slot, &blob(slot)).expect("lands");
    }
    assert_eq!(other.fence(0, 3, "node-3", 5), Ok(3));
    let mine =
        NodeLogStore::new(Arc::clone(&inner) as Arc<dyn BlobStore>, "", "node-1").expect("valid");
    assert_eq!(mine.fence(0, 0, "node-2", 4), Ok(3), "shared, not added to");
    assert_eq!(mine.read_entry(0, 4), Ok(None));
}
