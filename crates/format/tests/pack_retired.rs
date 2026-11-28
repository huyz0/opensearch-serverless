//! docs/specs/pack-retirement AC-8: the record of a departed node's packs.

use oss_format::{FormatError, PackRetired, pack_retired_key};

#[test]
fn the_record_is_under_the_retired_prefix() {
    assert_eq!(
        pack_retired_key("", "node-7").unwrap(),
        "packs/index/retired/node-7"
    );
    assert_eq!(
        pack_retired_key("b/", "n").unwrap(),
        "b/packs/index/retired/n"
    );
    assert!(pack_retired_key("", "a/b").is_err());
}

/// What was listed becomes a sidecar that marks every ordinal not listed.
#[test]
fn a_listing_marks_every_ordinal_it_did_not_see() {
    let r = PackRetired::listed(&[(3, 0), (3, 20), (5, 2)]).expect("builds");
    assert!(!r.is_over_cap());
    let epochs = r.sidecar().epochs();
    assert_eq!(epochs.len(), 2);
    assert_eq!((epochs[0].epoch, epochs[0].high), (3, 21));
    assert_eq!(epochs[0].unmarked().collect::<Vec<_>>(), vec![0, 20]);
    assert_eq!((epochs[1].epoch, epochs[1].high), (5, 3));
    assert_eq!(epochs[1].unmarked().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn an_empty_listing_and_an_over_cap_record_round_trip() {
    for r in [
        PackRetired::listed(&[]).unwrap(),
        PackRetired::listed(&[(1, 1023)]).unwrap(),
        PackRetired::over_cap(),
    ] {
        assert_eq!(PackRetired::parse(&r.encode().unwrap()).unwrap(), r);
    }
    assert!(PackRetired::over_cap().is_over_cap());
    assert!(PackRetired::over_cap().sidecar().epochs().is_empty());
}

#[test]
fn the_bytes_are_pinned() {
    assert_eq!(PackRetired::over_cap().encode().unwrap(), [1, 1]);
    let empty = PackRetired::listed(&[]).unwrap().encode().unwrap();
    assert_eq!(empty, [1, 0, 1, 0, 0, 0, 0]);
}

#[test]
fn an_ordinal_past_an_epoch_is_refused() {
    assert!(PackRetired::listed(&[(1, 1024)]).is_err());
}

#[test]
fn truncated_and_corrupt_records_are_refused() {
    assert!(matches!(
        PackRetired::parse(&[]),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(matches!(
        PackRetired::parse(&[2, 0]),
        Err(FormatError::Unsupported { .. })
    ));
    assert!(PackRetired::parse(&[1, 9]).is_err());
    // An over-cap record carries nothing after its state.
    assert!(PackRetired::parse(&[1, 1, 0]).is_err());
    let mut good = PackRetired::listed(&[(1, 1)]).unwrap().encode().unwrap();
    good.pop();
    assert!(PackRetired::parse(&good).is_err());
}
