//! docs/specs/stock-snapshot-restore AC-9: what restoring the recorded
//! 3.10 repository costs, the node's store and the source's together.

use oss_blob::{BlobStore, S3Op};
use oss_testkit::budget;

use crate::snapshot_restore::{requests, send};
use crate::stock_restore::{RESTORE, S3_REPO, s3_rig, s3_source_of};

fn all(store: &oss_blob::Emulator) -> u64 {
    S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum()
}

#[test]
fn a_stock_restore_costs_what_the_budget_says() {
    let (source, server) = s3_source_of("stock");
    let r = s3_rig(&server, "stocksrc/stock");
    assert_eq!(
        send(&r, "PUT", "/_snapshot/stock", Some(S3_REPO)).status,
        200
    );
    let (own, from) = (requests(&r), all(&source));
    assert_eq!(send(&r, "POST", RESTORE, None).status, 200);
    let detail = format!("node {} source {}", requests(&r) - own, all(&source) - from);
    budget::check(
        "snapshot.stock_restore.requests",
        (requests(&r) - own) + (all(&source) - from),
        &detail,
    );
}

#[test]
fn a_failed_stock_restore_costs_what_the_budget_says() {
    let (source, server) = s3_source_of("stock");
    // One of the two files' bytes changed: its length is right and its checksum is not.
    let key = "stock/repo/indices/M3IuI3tpQ7mcpIwUIA1kfQ/0/__L7JGf21KRju_ZczIDWC4kA";
    let mut bytes = Vec::new();
    source
        .get(
            key,
            oss_blob::ByteRange::Full,
            &mut [0u8; 64],
            &mut |c: &[u8]| {
                bytes.extend_from_slice(c);
                Ok(())
            },
        )
        .unwrap();
    bytes[3] ^= 1;
    source
        .put(key, &mut oss_blob::SliceSource::new(&bytes), &mut [0u8; 64])
        .unwrap();
    let r = s3_rig(&server, "stocksrc/stock");
    assert_eq!(
        send(&r, "PUT", "/_snapshot/stock", Some(S3_REPO)).status,
        200
    );
    let (own, from) = (requests(&r), all(&source));
    assert!(send(&r, "POST", RESTORE, None).status >= 400);
    let detail = format!("node {} source {}", requests(&r) - own, all(&source) - from);
    budget::check(
        "snapshot.stock_restore_cleanup.requests",
        (requests(&r) - own) + (all(&source) - from),
        &detail,
    );
}
