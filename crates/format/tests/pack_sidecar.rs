//! The collector's sidecar register: per live epoch, a bitmap of the ordinals
//! already collected, and the epoch's `high`.

use oss_format::{
    FormatError, MAX_SIDECAR_EPOCHS, ORDINALS_PER_EPOCH, PackSidecar, SIDECAR_EPOCH_BYTES,
    SidecarEpoch,
};

fn epoch(n: u64, high: u64) -> SidecarEpoch {
    SidecarEpoch::new(n, high)
}

/// AC-3: a sidecar round-trips, an epoch costs 144 bytes, and the cap holds
/// about 1,800 epochs under 256 KiB.
#[test]
fn a_sidecar_round_trips_at_144_bytes_an_epoch() {
    let mut a = epoch(1000, 1024);
    a.mark(0);
    a.mark(1023);
    let sidecar = PackSidecar::new(vec![a, epoch(1001, 5)]);
    let bytes = sidecar.encode().expect("encodes");
    assert_eq!(SIDECAR_EPOCH_BYTES, 144);
    assert_eq!(bytes.len(), 5 + 2 * SIDECAR_EPOCH_BYTES);
    assert_eq!(PackSidecar::parse(&bytes).expect("parses"), sidecar);
    const { assert!(MAX_SIDECAR_EPOCHS * SIDECAR_EPOCH_BYTES + 5 <= 256 << 10) };
    assert_eq!(MAX_SIDECAR_EPOCHS, 1_800);
    let largest = PackSidecar::new(
        (0..MAX_SIDECAR_EPOCHS as u64)
            .map(|n| epoch(n, 0))
            .collect(),
    );
    assert_eq!(largest.encode().expect("at the cap").len(), 5 + 1_800 * 144);
}

/// An ordinal is marked once; a full epoch (every ordinal marked) says so,
/// and the unmarked ordinals below `high` are the live packs to re-read.
#[test]
fn marks_name_the_ordinals_still_to_read() {
    let mut e = epoch(7, 10);
    for ordinal in [0, 1, 2, 4, 9] {
        e.mark(ordinal);
    }
    assert!(e.is_marked(4) && !e.is_marked(3));
    assert_eq!(e.unmarked().collect::<Vec<_>>(), vec![3, 5, 6, 7, 8]);
    assert!(!e.is_full());
    let mut all = epoch(8, ORDINALS_PER_EPOCH);
    for ordinal in 0..ORDINALS_PER_EPOCH {
        all.mark(ordinal);
    }
    assert!(all.is_full());
    assert_eq!(all.unmarked().count(), 0);
    // An ordinal past the epoch is ignored, never a panic.
    all.mark(ORDINALS_PER_EPOCH + 5);
    assert!(!all.is_marked(ORDINALS_PER_EPOCH + 5));
}

/// A value that is short, of another version, of the wrong length or past
/// the cap is refused; so is an epoch count past the cap on encode.
#[test]
fn corrupt_values_are_refused() {
    let good = PackSidecar::new(vec![epoch(1, 1)])
        .encode()
        .expect("encodes");
    assert!(matches!(
        PackSidecar::parse(&[]),
        Err(FormatError::Corrupt { .. })
    ));
    let mut other = good.clone();
    other[0] = 9;
    assert!(matches!(
        PackSidecar::parse(&other),
        Err(FormatError::Unsupported { .. })
    ));
    assert!(matches!(
        PackSidecar::parse(&good[..good.len() - 1]),
        Err(FormatError::Corrupt { .. })
    ));
    let mut extra = good.clone();
    extra.push(0);
    assert!(matches!(
        PackSidecar::parse(&extra),
        Err(FormatError::Corrupt { .. })
    ));
    let too_many = (0..=MAX_SIDECAR_EPOCHS as u64)
        .map(|n| epoch(n, 0))
        .collect::<Vec<_>>();
    assert!(PackSidecar::new(too_many).encode().is_err());
}

/// A full epoch is dropped, except the newest, which is never dropped.
#[test]
fn full_epochs_are_dropped_but_the_newest() {
    let full = |n| {
        let mut e = epoch(n, ORDINALS_PER_EPOCH);
        for o in 0..ORDINALS_PER_EPOCH {
            e.mark(o);
        }
        e
    };
    let mut s = PackSidecar::new(vec![full(1), epoch(2, 3), full(3)]);
    s.drop_full();
    let kept: Vec<u64> = s.epochs().iter().map(|e| e.epoch).collect();
    assert_eq!(kept, vec![2, 3]);
}
