//! The indexing-memory controller: the budget a setting resolves to, when a
//! check is due, and what a check asks of the shards.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_writer::indexing_memory::{
    BufferSize, DEFAULT_INACTIVE_MILLIS, DEFAULT_MIN_BYTES, DEFAULT_PERCENT_BASIS_POINTS, Decision,
    IndexingBudget, IndexingMemory, ShardUsage,
};

const MIB: u64 = 1 << 20;

fn usage(key: &'static str, buffer: u64, writing: u64, idle: u64) -> ShardUsage<&'static str> {
    ShardUsage {
        key,
        buffer_bytes: buffer,
        writing_bytes: writing,
        idle_millis: idle,
    }
}

fn controller(budget: u64) -> IndexingMemory<&'static str> {
    IndexingMemory::new(
        IndexingBudget::resolve(BufferSize::Bytes(budget), 0, 0, None),
        DEFAULT_INACTIVE_MILLIS,
    )
}

#[test]
fn a_percentage_is_a_share_of_the_heap_with_a_floor_and_a_ceiling() {
    let ten = BufferSize::Percent(DEFAULT_PERCENT_BASIS_POINTS);
    let resolve = |size, heap, min, max| IndexingBudget::resolve(size, heap, min, max).bytes();
    assert_eq!(
        resolve(ten, 4096 * MIB, DEFAULT_MIN_BYTES, None),
        4096 * MIB / 10
    );
    assert_eq!(
        resolve(ten, 100 * MIB, DEFAULT_MIN_BYTES, None),
        DEFAULT_MIN_BYTES,
        "floor"
    );
    assert_eq!(
        resolve(ten, 4096 * MIB, DEFAULT_MIN_BYTES, Some(100 * MIB)),
        100 * MIB,
        "ceiling"
    );
    assert_eq!(
        resolve(BufferSize::Percent(1250), 800 * MIB, 0, None),
        100 * MIB,
        "12.5%"
    );
    // Bytes are used as they are.
    assert_eq!(
        resolve(
            BufferSize::Bytes(MIB),
            4096 * MIB,
            DEFAULT_MIN_BYTES,
            Some(10)
        ),
        MIB
    );
    assert_eq!(
        resolve(BufferSize::Bytes(0), 4096 * MIB, DEFAULT_MIN_BYTES, None),
        0
    );
}

#[test]
fn a_check_is_due_when_a_thirtieth_of_the_budget_has_been_written() {
    let m = controller(3000);
    assert!(!m.bytes_written(60));
    assert!(!m.bytes_written(40), "exactly a thirtieth is not past it");
    assert_eq!(m.pending_bytes(), 100);
    assert!(m.bytes_written(1), "past it");
    assert_eq!(m.pending_bytes(), 0, "the count starts again");
    assert!(!m.bytes_written(100));
    assert!(m.bytes_written(101));
}

#[test]
fn racing_writers_ask_for_no_more_checks_than_the_bytes_allow() {
    let m = Arc::new(controller(3000));
    let due = Arc::new(AtomicU64::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (m, due) = (Arc::clone(&m), Arc::clone(&due));
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    if m.bytes_written(1) {
                        due.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
    let due = due.load(Ordering::SeqCst);
    assert!(
        (1..=80).contains(&due),
        "{due} checks for 8000 bytes at 100 a check"
    );
    assert!(m.pending_bytes() <= 8000);
}

#[test]
fn under_budget_refreshes_nothing() {
    let m = controller(100);
    let d = m.check(&[usage("a", 60, 0, 0), usage("b", 40, 0, 0)]);
    assert_eq!(d, Decision::default());
}

#[test]
fn over_budget_refreshes_the_largest_until_the_rest_fits() {
    let m = controller(100);
    let d = m.check(&[
        usage("small", 20, 0, 0),
        usage("big", 70, 0, 0),
        usage("mid", 40, 0, 0),
    ]);
    // 130 used: the biggest (70) brings it to 60, which fits.
    assert_eq!(d.refresh, ["big"]);
    assert!(d.throttle.is_empty() && d.release.is_empty() && d.flush_idle.is_empty());
    let d = m.check(&[
        usage("a", 70, 0, 0),
        usage("b", 60, 0, 0),
        usage("c", 50, 0, 0),
    ]);
    // 180 used: 70 leaves 110, which is over; 60 more leaves 50.
    assert_eq!(d.refresh, ["a", "b"]);
}

#[test]
fn a_shard_with_nothing_buffered_is_never_refreshed() {
    let m = controller(10);
    let d = m.check(&[usage("empty", 0, 0, 0), usage("full", 50, 0, 0)]);
    assert_eq!(d.refresh, ["full"]);
}

#[test]
fn bytes_being_written_are_not_counted_twice() {
    let m = controller(100);
    // 90 buffered of which 50 are being written: 40 counted.
    let d = m.check(&[usage("a", 90, 50, 0), usage("b", 50, 0, 0)]);
    assert_eq!(d.refresh, Vec::<&str>::new(), "40 + 50 fits 100");
    // Writing more than buffered counts as none.
    let d = m.check(&[usage("a", 10, 60, 0), usage("b", 90, 0, 0)]);
    assert!(d.refresh.is_empty(), "{d:?}");
    let d = m.check(&[usage("a", 10, 60, 0), usage("b", 101, 0, 0)]);
    assert_eq!(d.refresh, ["b"]);
}

#[test]
fn past_one_and_a_half_budgets_the_refreshed_are_throttled_once() {
    let m = controller(100);
    let busy = [usage("a", 100, 40, 0), usage("b", 100, 0, 0)];
    // 60 + 100 counted and 40 writing: 200 > 150, and 160 > 100.
    let d = m.check(&busy);
    assert_eq!(d.refresh, ["b"]);
    assert_eq!(d.throttle, ["b"]);
    // The same pressure again: b is refreshed again but throttled already.
    let d = m.check(&busy);
    assert_eq!(d.refresh, ["b"]);
    assert!(d.throttle.is_empty(), "{d:?}");
    assert!(d.release.is_empty());
}

#[test]
fn easing_below_one_and_a_half_budgets_releases_every_throttled_shard() {
    let m = controller(100);
    let d = m.check(&[usage("a", 200, 0, 0), usage("b", 100, 0, 0)]);
    assert_eq!(d.throttle, ["a"]);
    // Exactly one and a half budgets is not past it.
    let d = m.check(&[usage("a", 150, 0, 0)]);
    assert_eq!(d.refresh, ["a"]);
    assert!(d.throttle.is_empty());
    assert_eq!(d.release, ["a"], "released: 150 is not past 150");
    let d = m.check(&[usage("a", 10, 0, 0)]);
    assert!(d.release.is_empty(), "none left to release");
}

#[test]
fn a_shard_idle_for_the_inactive_time_is_flushed() {
    let m = controller(1000);
    let d = m.check(&[
        usage("busy", 10, 0, DEFAULT_INACTIVE_MILLIS - 1),
        usage("quiet", 10, 0, DEFAULT_INACTIVE_MILLIS),
        usage("gone", 10, 0, DEFAULT_INACTIVE_MILLIS + 1),
    ]);
    assert_eq!(d.flush_idle, ["quiet", "gone"]);
}

#[test]
fn the_controller_names_its_budget() {
    let m = controller(777);
    assert_eq!(m.budget().bytes(), 777);
    assert!(format!("{m:?}").contains("777"));
}
