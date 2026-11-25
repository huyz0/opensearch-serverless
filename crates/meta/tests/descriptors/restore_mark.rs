//! docs/specs/snapshot-restore AC-9: an index a restore has not finished is
//! frozen, snapshotted and aliased by none, and a delete still clears it.

use oss_format::{IndexRecord, mark_restoring};
use oss_meta::{AliasRecord, CaptureError, DeleteMode, MetaError, SnapshotTaker};

use crate::alias_mutations::{at_now, mutations};
use crate::point_in_time::buf;
use crate::point_in_time_capture::{KEEP_ALIVE, setup};
use crate::{BASE, all};

fn mark(d: &oss_meta::DescriptorStore, name: &str) {
    let marked = mark_restoring(None).unwrap();
    let mut record = IndexRecord::new(name, "u1", 1);
    record.settings = &marked;
    d.create(&record, 1, &mut [0u8; 4096]).unwrap();
}

#[test]
fn a_marked_index_is_neither_frozen_nor_snapshotted() {
    for b in all() {
        let (d, p) = setup(&b);
        mark(&d, "logs");
        assert!(
            matches!(
                p.capture(&d, "logs", KEEP_ALIVE, &mut buf()),
                Err(CaptureError::Restoring)
            ),
            "{:?}",
            b.kind
        );
        let taker = SnapshotTaker::new(std::sync::Arc::clone(&b.store), BASE);
        assert!(
            matches!(
                taker.describe(&d, "logs", &mut buf()),
                Err(CaptureError::Restoring)
            ),
            "{:?}",
            b.kind
        );
    }
}

#[test]
fn a_marked_index_takes_no_alias_and_is_deleted_as_any() {
    let b = &all()[0];
    let d = b.descriptors(DeleteMode::Conditional);
    mark(&d, "logs");
    let mut m = mutations(&d, &at_now);
    let err = m.create_alias(&AliasRecord::new("al", &["logs"]));
    assert!(matches!(err, Err(MetaError::InvalidArgument(_))), "{err:?}");
    d.delete("logs").unwrap();
    assert!(d.get("logs", &mut buf()).unwrap().is_none());
}
