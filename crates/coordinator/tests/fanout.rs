//! docs/specs/coordinator-fanout: shards are asked concurrently and coverage is reported.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use std::time::Duration;

use oss_coordinator::fanout::{Coverage, TaskError, gather, run};

#[test]
fn outcomes_come_in_item_order_whichever_finished_first() {
    let items = [30u64, 1, 20, 2, 10];
    let out = run(4, &items, |ms| {
        thread::sleep(Duration::from_millis(*ms));
        Ok::<u64, ()>(*ms)
    });
    let got: Vec<u64> = out.into_iter().map(|r| r.expect("ok")).collect();
    assert_eq!(got, items);
}

#[test]
fn no_more_than_the_concurrency_are_in_flight() {
    for concurrency in [1usize, 3] {
        let now = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let items: Vec<u32> = (0..12).collect();
        run(concurrency, &items, |_| {
            let inside = now.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(inside, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(5));
            now.fetch_sub(1, Ordering::SeqCst);
            Ok::<(), ()>(())
        });
        assert!(peak.load(Ordering::SeqCst) <= concurrency);
    }
    // Zero is one at a time, and one is the caller's own thread alone.
    let me = thread::current().id();
    let seen: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
    for concurrency in [0usize, 1] {
        run(concurrency, &[1, 2, 3], |_| {
            seen.lock().expect("lock").push(thread::current().id());
            Ok::<(), ()>(())
        });
    }
    let seen = seen.into_inner().expect("lock");
    assert_eq!(seen.len(), 6);
    assert!(seen.iter().all(|id| *id == me));
}

#[test]
fn a_wide_window_uses_more_than_one_thread() {
    let ids: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
    let items: Vec<u32> = (0..8).collect();
    run(4, &items, |_| {
        ids.lock().expect("lock").push(thread::current().id());
        thread::sleep(Duration::from_millis(20));
        Ok::<(), ()>(())
    });
    let mut ids = ids.into_inner().expect("lock");
    ids.sort_by_key(|id| format!("{id:?}"));
    ids.dedup();
    assert!(ids.len() > 1, "work stayed on one thread");
}

#[test]
fn a_failed_call_fails_only_its_own_slot() {
    let out = run(
        3,
        &[1, 2, 3, 4],
        |n| if *n == 2 { Err("no") } else { Ok(*n) },
    );
    assert_eq!(out[0], Ok(1));
    assert_eq!(out[1], Err(TaskError::Failed("no")));
    assert_eq!(out[2], Ok(3));
    assert_eq!(out[3], Ok(4));
}

#[test]
fn a_panicking_call_fails_only_its_own_slot() {
    let out = run(3, &[1, 2, 3, 4], |n| {
        assert!(*n != 3, "boom");
        Ok::<i32, ()>(*n)
    });
    assert_eq!(out[0], Ok(1));
    assert_eq!(out[1], Ok(2));
    assert_eq!(out[2], Err(TaskError::Panicked));
    assert_eq!(out[3], Ok(4));
}

#[test]
fn gather_splits_answers_failures_and_counts_coverage() {
    let outcomes = vec![
        Ok("a"),
        Err(TaskError::Failed("x")),
        Ok("b"),
        Err(TaskError::Panicked),
    ];
    let got = gather(2, outcomes).expect("some answered");
    assert_eq!(got.answers, ["a", "b"]);
    assert_eq!(
        got.failures,
        vec![(1, TaskError::Failed("x")), (3, TaskError::Panicked)]
    );
    assert_eq!(
        got.coverage,
        Coverage {
            total: 6,
            successful: 4,
            skipped: 2,
            failed: 2
        }
    );
}

#[test]
fn nobody_answering_gives_the_lowest_failure_and_nothing_gives_nothing() {
    let outcomes: Vec<Result<u8, TaskError<&str>>> = vec![
        Err(TaskError::Failed("first")),
        Err(TaskError::Failed("second")),
    ];
    assert_eq!(
        gather(0, outcomes).err(),
        Some((0, TaskError::Failed("first")))
    );
    let none: Vec<Result<u8, TaskError<&str>>> = Vec::new();
    let got = gather(3, none).expect("empty is not a failure");
    assert!(got.answers.is_empty());
    assert_eq!(
        got.coverage,
        Coverage {
            total: 3,
            successful: 3,
            skipped: 3,
            failed: 0
        }
    );
}
