//! AC-2: of racing writers from one generation, exactly one applies.

use std::sync::{Arc, Barrier};

use oss_blob::SwapOutcome;

use crate::{all, read};

const WRITERS: usize = 8;

/// Runs `write(i)` on `WRITERS` threads at once and returns what each got.
fn race(write: impl Fn(usize) -> SwapOutcome + Send + Sync + 'static) -> Vec<SwapOutcome> {
    let write = Arc::new(write);
    let barrier = Arc::new(Barrier::new(WRITERS));
    let threads: Vec<_> = (0..WRITERS)
        .map(|i| {
            let (write, barrier) = (Arc::clone(&write), Arc::clone(&barrier));
            std::thread::spawn(move || {
                barrier.wait();
                write(i)
            })
        })
        .collect();
    threads.into_iter().map(|t| t.join().unwrap()).collect()
}

/// The one writer whose write applied.
fn winner(outcomes: &[SwapOutcome]) -> (usize, u64) {
    let applied: Vec<_> = outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, o)| match o {
            SwapOutcome::Applied(g) => Some((i, *g)),
            SwapOutcome::Conflict(_) => None,
        })
        .collect();
    assert_eq!(applied.len(), 1, "{outcomes:?}");
    applied[0]
}

#[test]
fn one_of_racing_swaps_applies_and_its_value_is_stored() {
    for b in all() {
        let SwapOutcome::Applied(g) = b.store.create_register("race/swap", b"start").unwrap()
        else {
            panic!("create refused");
        };
        let store = Arc::clone(&b.store);
        let outcomes = race(move |i| {
            store
                .swap_register("race/swap", g, format!("writer {i}").as_bytes())
                .unwrap()
        });
        let (i, next) = winner(&outcomes);
        assert_eq!(next, g + 1, "{:?}", b.kind);
        assert_eq!(
            read(&*b.store, "race/swap"),
            Some((next, format!("writer {i}").into_bytes())),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn one_of_racing_creates_applies_and_its_value_is_stored() {
    for b in all() {
        let store = Arc::clone(&b.store);
        let outcomes = race(move |i| {
            store
                .create_register("race/create", format!("writer {i}").as_bytes())
                .unwrap()
        });
        let (i, g) = winner(&outcomes);
        assert_eq!(
            read(&*b.store, "race/create"),
            Some((g, format!("writer {i}").into_bytes())),
            "{:?}",
            b.kind
        );
    }
}
