//! Change stripes written byte for byte as the Java's `ChangeStripes`
//! writes them (docs/specs/change-stripes/SPEC.md, AC-1 and AC-2).

// ARITH: counts and generations here are small test constants.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use common::stripe_fixture;
use oss_format::ChangeKind::{Changed, Deleted, PendingDelete};
use oss_format::{
    ChangeKind, ChangeStripe, FormatError, KEPT, KEPT_PENDING, MAX_CHANGE_STRIPE_BYTES,
    MAX_ENTRY_NAME_BYTES, STRIPES, StripeEntry, change_stripe_key, stripe_of,
};

fn written(stripe: &ChangeStripe<'_>) -> Vec<u8> {
    let mut buf = vec![0u8; MAX_CHANGE_STRIPE_BYTES];
    let n = stripe.write(&mut buf).unwrap();
    buf.truncate(n);
    buf
}

fn replay<'a>(bumps: &[(&'a str, ChangeKind)]) -> ChangeStripe<'a> {
    let mut stripe = ChangeStripe::default();
    for (name, kind) in bumps {
        stripe.add(name, *kind);
    }
    stripe
}

#[test]
fn bumps_write_the_javas_bytes() {
    let stripe = replay(&[
        ("alpha", Changed),
        ("beta", PendingDelete),
        ("beta", Changed),
        ("gamma été 日本", Changed),
        ("alpha", PendingDelete),
        ("beta", Deleted),
    ]);
    assert_eq!(written(&stripe), stripe_fixture("bumps.txt"));
    assert_eq!(
        stripe.entries[3],
        StripeEntry {
            generation: 5,
            pending: true,
            name: "alpha"
        }
    );
}

#[test]
fn a_delete_finishes_only_its_own_pending_entry() {
    let stripe = replay(&[("x", PendingDelete), ("y", PendingDelete), ("x", Deleted)]);
    assert_eq!(
        stripe.entries,
        [
            StripeEntry {
                generation: 2,
                pending: true,
                name: "y"
            },
            StripeEntry {
                generation: 3,
                pending: false,
                name: "x"
            },
        ]
    );
}

#[test]
fn only_the_newest_applied_changes_are_kept_with_every_pending_delete() {
    let names: Vec<String> = (0..300).map(|i| format!("n-{i}")).collect();
    let mut stripe = ChangeStripe::default();
    stripe.add(&names[0], Changed);
    stripe.add("kept", PendingDelete);
    for name in &names[1..] {
        stripe.add(name, Changed);
    }
    assert_eq!(written(&stripe), stripe_fixture("trimmed.txt"));
    assert_eq!(stripe.entries.len(), KEPT + 1);
    assert_eq!(
        (stripe.entries[0].name, stripe.entries[0].pending),
        ("kept", true)
    );
}

#[test]
fn too_many_pending_deletes_start_a_new_epoch() {
    let names: Vec<String> = (0..=KEPT_PENDING).map(|i| format!("gone-{i}")).collect();
    let mut stripe = ChangeStripe::default();
    for name in &names {
        stripe.add(name, PendingDelete);
    }
    assert_eq!((stripe.epoch, stripe.generation), (1, 65));
    assert!(stripe.entries.is_empty());
    stripe.add("after", Changed);
    assert_eq!(written(&stripe), stripe_fixture("new-epoch.txt"));
}

#[test]
fn what_the_java_writes_reads_back_and_writes_again() {
    for file in ["bumps.txt", "trimmed.txt", "new-epoch.txt", "reencoded.txt"] {
        let bytes = stripe_fixture(file);
        let stripe = ChangeStripe::parse(&bytes).unwrap();
        assert_eq!(written(&stripe), bytes, "{file}");
    }
    // As leniently as the Java: a trailing space in the head, blank lines,
    // a name holding a space, and an unknown kind read as applied.
    let stripe = ChangeStripe::parse(b"v1 3 9 \n\n5 D a b\n7 X c\n9 P d\n\n").unwrap();
    assert_eq!((stripe.epoch, stripe.generation), (3, 9));
    assert_eq!(
        stripe.entries,
        [
            StripeEntry {
                generation: 5,
                pending: false,
                name: "a b"
            },
            StripeEntry {
                generation: 7,
                pending: false,
                name: "c"
            },
            StripeEntry {
                generation: 9,
                pending: true,
                name: "d"
            },
        ]
    );
    assert_eq!(written(&stripe), stripe_fixture("reencoded.txt"));
    // A name may be empty, as the Java's split leaves it.
    assert_eq!(
        ChangeStripe::parse(b"v1 0 1\n1 D \n").unwrap().entries[0].name,
        ""
    );
    assert_eq!(ChangeStripe::parse(b"v1 +1 -2").unwrap().generation, -2);
}

#[test]
fn a_malformed_stripe_is_refused() {
    for bad in [
        &b""[..],
        b"\nv1 0 0",
        b"v2 0 0",
        b"v1 0",
        b"v1 0 0 0",
        b"v1  0 0",
        b"v1 x 0",
        b"v1 0 0x",
        b"v1 0 1\n1 D",
        b"v1 0 1\nx D a",
        b"v1 0 1\n1",
        b"v1 0 \xff",
    ] {
        assert!(
            matches!(ChangeStripe::parse(bad), Err(FormatError::Corrupt { .. })),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn the_longest_stripe_fits_its_bound() {
    let names: Vec<String> = (0..KEPT + KEPT_PENDING)
        .map(|i| format!("{i:0>width$}", width = MAX_ENTRY_NAME_BYTES))
        .collect();
    let mut stripe = ChangeStripe {
        epoch: i64::MIN,
        generation: i64::MIN,
        entries: Vec::new(),
    };
    for (i, name) in names.iter().enumerate() {
        stripe.entries.push(StripeEntry {
            generation: i64::MIN,
            pending: i < KEPT_PENDING,
            name,
        });
    }
    assert_eq!(written(&stripe).len(), MAX_CHANGE_STRIPE_BYTES);
    let mut short = vec![0u8; MAX_CHANGE_STRIPE_BYTES - 1];
    assert_eq!(
        stripe.write(&mut short),
        Err(FormatError::TooLarge {
            what: "change stripe",
            limit: (MAX_CHANGE_STRIPE_BYTES - 1) as u64,
            actual: MAX_CHANGE_STRIPE_BYTES as u64,
        })
    );
}

#[test]
fn a_name_hashes_to_the_javas_stripe() {
    let table = stripe_fixture("stripe-of.txt");
    let table = String::from_utf8(table).unwrap();
    for line in table.lines() {
        let (stripe, name) = line.split_once(' ').unwrap();
        assert_eq!(stripe_of(name).to_string(), stripe, "{name:?}");
    }
    assert!((0..1000).all(|i| stripe_of(&format!("n{i}")) < STRIPES));
}

#[test]
fn each_stripe_has_its_key() {
    assert_eq!(change_stripe_key("", 0).unwrap(), "index-changes/stripe-00");
    assert_eq!(
        change_stripe_key("d/", 15).unwrap(),
        "d/index-changes/stripe-15"
    );
    assert!(change_stripe_key("d/", STRIPES).is_err());
    assert!(change_stripe_key("d", 0).is_err());
}
