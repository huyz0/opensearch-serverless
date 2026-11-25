//! The blob-store contract, checked against the in-process emulator
//! (docs/specs/blob-store/SPEC.md). Every test asserts exact request counts.

mod deletes;
mod faults;
mod listing;
mod reads;
mod request_cost;
mod usage;
mod writes;

use oss_blob::{BlobResult, BlobStore, ByteRange, Emulator, Meter, S3Op, SliceSource};

pub(crate) const MIB: u64 = 1 << 20;

pub(crate) fn emulator() -> Emulator {
    Emulator::new(64 * MIB, 1)
}

/// Asserts the meter holds exactly `expected` attempts and nothing else.
pub(crate) fn assert_counts(meter: &Meter, expected: &[(S3Op, u64)]) {
    for op in S3Op::ALL {
        let want = expected
            .iter()
            .find(|(o, _)| *o == op)
            .map_or(0, |(_, n)| *n);
        assert_eq!(meter.count(op), want, "{op:?}");
    }
}

pub(crate) fn put(store: &dyn BlobStore, key: &str, bytes: &[u8]) {
    store
        .put(key, &mut SliceSource::new(bytes), &mut [0u8; 4096])
        .unwrap();
}

pub(crate) fn get_all(store: &dyn BlobStore, key: &str, range: ByteRange) -> BlobResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut sink = |c: &[u8]| {
        out.extend_from_slice(c);
        Ok(())
    };
    store.get(key, range, &mut [0u8; 1000], &mut sink)?;
    Ok(out)
}

pub(crate) fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}
