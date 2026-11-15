//! AC-3 and AC-4: the requests each register operation sends, per backend,
//! and a remembered version gone stale.

use oss_blob::S3Op::{Delete, Get, GetRange, Put};
use oss_blob::{S3Op, SwapOutcome};

use crate::{Kind, all, counted, plant, read};

fn applied(outcome: SwapOutcome) -> u64 {
    match outcome {
        SwapOutcome::Applied(g) => g,
        SwapOutcome::Conflict(seen) => panic!("conflict, saw {seen:?}"),
    }
}

// AC-3
#[test]
fn each_operation_sends_what_the_spec_budgets() {
    for b in all() {
        let s = &*b.store;
        let m = s.meter();
        let fs = b.kind == Kind::Fs;
        let budget = |object: &[(S3Op, u64)], file: &[(S3Op, u64)]| {
            if fs { file.to_vec() } else { object.to_vec() }
        };

        let (g, n) = counted(m, || applied(s.create_register("c/k", b"v1").unwrap()));
        assert_eq!(n, [(Put, 1)], "{:?} create", b.kind);
        let (_, n) = counted(m, || read(s, "c/k"));
        assert_eq!(n, [(Get, 1)], "{:?} read", b.kind);
        // The version just written is remembered: no read before the PUT.
        let (g, n) = counted(m, || applied(s.swap_register("c/k", g, b"v2").unwrap()));
        assert_eq!(
            n,
            budget(&[(Put, 1)], &[(GetRange, 1), (Put, 1)]),
            "{:?} swap, remembered",
            b.kind
        );

        // Written by someone else: read the generation first.
        plant(s, "c/k", g + 10, b"theirs");
        let (_, n) = counted(m, || {
            applied(s.swap_register("c/k", g + 10, b"v3").unwrap())
        });
        assert_eq!(
            n,
            [(GetRange, 1), (Put, 1)],
            "{:?} swap, not remembered",
            b.kind
        );
        let (_, n) = counted(m, || s.swap_register("c/k", g, b"stale").unwrap());
        assert_eq!(n, [(GetRange, 1)], "{:?} swap, stale generation", b.kind);

        // Remembered after the read: one DELETE.
        let (seen, _) = counted(m, || read(s, "c/k"));
        let (deleted, n) = counted(m, || s.delete_register("c/k", seen.unwrap().0).unwrap());
        assert!(deleted);
        assert_eq!(
            n,
            budget(&[(Delete, 1)], &[(GetRange, 1), (Delete, 1)]),
            "{:?} delete, remembered",
            b.kind
        );

        plant(s, "c/k", 77, b"x");
        let (deleted, n) = counted(m, || s.delete_register("c/k", 77).unwrap());
        assert!(deleted);
        assert_eq!(
            n,
            [(GetRange, 1), (Delete, 1)],
            "{:?} delete, not remembered",
            b.kind
        );
        let (deleted, n) = counted(m, || s.delete_register("c/k", 77).unwrap());
        assert!(!deleted);
        assert_eq!(n, [(GetRange, 1)], "{:?} delete, absent", b.kind);
    }
}

// AC-4
#[test]
fn a_stale_remembered_version_costs_one_refused_put_never_a_wrong_write() {
    for b in all() {
        let s = &*b.store;
        let m = s.meter();
        let g = applied(s.create_register("s/k", b"mine").unwrap());
        // Another writer replaces the register, at the same generation the
        // store remembers, so only the ETag tells the versions apart.
        plant(s, "s/k", g, b"theirs");
        let (outcome, n) = counted(m, || s.swap_register("s/k", g, b"lost").unwrap());
        if b.kind == Kind::Fs {
            // The filesystem holds the lock and compares generations: equal
            // generations swap, as the Java's would.
            assert_eq!(outcome, SwapOutcome::Applied(g + 1));
            continue;
        }
        assert_eq!(outcome, SwapOutcome::Conflict(None), "{:?}", b.kind);
        assert_eq!(n, [(Put, 1)], "{:?}", b.kind);
        // The memory is gone: the next swap reads afresh, and replaces
        // the other writer's version only knowingly.
        let (outcome, n) = counted(m, || s.swap_register("s/k", g, b"next").unwrap());
        assert_eq!(outcome, SwapOutcome::Applied(g + 1), "{:?}", b.kind);
        assert_eq!(n, [(GetRange, 1), (Put, 1)], "{:?}", b.kind);
        assert_eq!(
            read(s, "s/k"),
            Some((g + 1, b"next".to_vec())),
            "{:?}",
            b.kind
        );
    }
}
