//! Injected faults and latency, and argument checks.

use std::time::{Duration, Instant};

use crate::{MIB, assert_counts, emulator};
use oss_blob::emulator::faults::{Faults, Latency};
use oss_blob::*;

// AC-9
#[test]
fn the_same_seed_places_the_same_faults() {
    let run = |seed: u64| {
        let e = Emulator::new(MIB, seed);
        e.set_faults(
            S3Op::Head,
            Faults {
                fail_ppm: 200_000,
                throttle_ppm: 200_000,
            },
        );
        (0..200).map(|_| e.head("k")).collect::<Vec<_>>()
    };
    let a = run(9);
    assert_eq!(a, run(9));
    assert_ne!(a, run(10));
    let failed = a
        .iter()
        .filter(|r| **r == Err(BlobError::Unavailable))
        .count();
    let throttled = a
        .iter()
        .filter(|r| **r == Err(BlobError::Throttled))
        .count();
    assert!(
        (20..=60).contains(&failed) && (20..=60).contains(&throttled),
        "{failed} {throttled}"
    );
}

#[test]
fn scripted_faults_hit_exactly_the_next_requests_of_their_kind() {
    let e = emulator();
    e.fail_next(S3Op::Put, 2);
    let src = || SliceSource::new(b"v");
    assert_eq!(
        e.put("k", &mut src(), &mut [0; 4]),
        Err(BlobError::Unavailable)
    );
    assert_eq!(e.head("k"), Ok(None), "another kind is untouched");
    assert_eq!(
        e.put("k", &mut src(), &mut [0; 4]),
        Err(BlobError::Unavailable)
    );
    assert_eq!(e.put("k", &mut src(), &mut [0; 4]), Ok(()));
    e.throttle_next(S3Op::Put, 1);
    assert_eq!(
        e.create_if_absent("j", &mut src(), &mut [0; 4]),
        Err(BlobError::Throttled)
    );
    assert_eq!(
        e.create_if_absent("j", &mut src(), &mut [0; 4]),
        Ok(CreateOutcome::Created)
    );
    assert_counts(e.meter(), &[(S3Op::Put, 5), (S3Op::Head, 1)]);
    assert_eq!(e.meter().errors(), 3);
}

// AC-10
#[test]
fn injected_latency_delays_each_request_of_its_kind() {
    let e = emulator();
    e.set_latency(
        S3Op::Head,
        Latency {
            base: Duration::from_millis(20),
            jitter: Duration::from_millis(5),
        },
    );
    let t = Instant::now();
    for _ in 0..3 {
        e.head("k").unwrap();
    }
    assert!(
        t.elapsed() >= Duration::from_millis(60),
        "{:?}",
        t.elapsed()
    );
    let t = Instant::now();
    e.delete("k").unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(20),
        "another kind is not delayed"
    );
}

// AC-11
#[test]
fn invalid_arguments_fail_before_any_request() {
    let e = emulator();
    let long = "k".repeat(1025);
    let bad = BlobError::InvalidArgument;
    let mut sink = |_: &[u8]| Ok(());
    assert!(matches!(
        e.get("k", ByteRange::Full, &mut [], &mut sink),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.get("", ByteRange::Full, &mut [0; 1], &mut sink),
        Err(BlobError::InvalidArgument(_))
    ));
    assert_eq!(
        e.get(
            "k",
            ByteRange::From { start: 0, len: 0 },
            &mut [0; 1],
            &mut sink
        ),
        Err(bad("range of 0 bytes"))
    );
    assert!(matches!(
        e.put(&long, &mut SliceSource::new(b""), &mut [0; 1]),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.put("k", &mut SliceSource::new(b""), &mut []),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.create_if_absent("", &mut SliceSource::new(b""), &mut [0; 1]),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(
        e.create_if_absent("k", &mut SliceSource::new(b""), &mut []),
        Err(BlobError::InvalidArgument(_))
    ));
    assert!(matches!(e.head(""), Err(BlobError::InvalidArgument(_))));
    assert!(matches!(
        e.delete(&long),
        Err(BlobError::InvalidArgument(_))
    ));
    // LIST: the operation under test.
    assert_eq!(
        e.list_prefix("", None, 0, &mut |_, _| {}),
        Err(bad("max_keys of 0"))
    );
    assert_eq!(e.meter().total(), 0);
    assert_eq!(e.meter().errors(), 0);
}
