//! The seeding register: where the one-time walk of `packs/` has got to.

use oss_format::{FormatError, PackSeed, pack_seed_key};

/// The register sits beside the index registers, under the base.
#[test]
fn the_register_is_packs_index_seeded_under_the_base() {
    assert_eq!(pack_seed_key(""), "packs/index/seeded");
    assert_eq!(pack_seed_key("base/"), "base/packs/index/seeded");
}

/// A state round-trips, in progress with its cursor and done without one.
#[test]
fn a_state_round_trips() {
    let begun = PackSeed::begun();
    assert!(!begun.done());
    assert_eq!(begun.cursor(), None);
    let moved = PackSeed::after("packs/node-7/");
    assert!(!moved.done());
    assert_eq!(moved.cursor(), Some("packs/node-7/"));
    for state in [begun, moved, PackSeed::finished()] {
        assert_eq!(
            PackSeed::parse(&state.encode().expect("encodes")).expect("parses"),
            state
        );
    }
    assert!(PackSeed::finished().done());
    assert_eq!(PackSeed::finished().cursor(), None);
}

/// The value is a version byte, a state byte, a cursor length (u16) and its
/// bytes, big-endian.
#[test]
fn the_bytes_are_pinned() {
    assert_eq!(PackSeed::begun().encode().unwrap(), [1, 0, 0, 0]);
    assert_eq!(PackSeed::finished().encode().unwrap(), [1, 1, 0, 0]);
    assert_eq!(
        PackSeed::after("ab").encode().unwrap(),
        [1, 0, 0, 2, b'a', b'b']
    );
}

/// What is not a state is refused: another version, an unknown state, a
/// cursor that overruns or leaves bytes, a finished state with a cursor, a
/// cursor past its bound.
#[test]
fn what_is_not_a_state_is_refused() {
    assert!(matches!(
        PackSeed::parse(&[2, 0, 0, 0]),
        Err(FormatError::Unsupported { .. })
    ));
    for bad in [
        &[][..],
        &[1, 0, 0][..],
        &[1, 2, 0, 0][..],
        &[1, 0, 0, 3, b'a'][..],
        &[1, 0, 0, 0, 9][..],
        &[1, 1, 0, 1, b'a'][..],
    ] {
        assert!(PackSeed::parse(bad).is_err(), "{bad:?}");
    }
    let long = "k".repeat(1025);
    assert!(PackSeed::after(&long).encode().is_err());
    assert!(PackSeed::after(&"k".repeat(1024)).encode().is_ok());
}
