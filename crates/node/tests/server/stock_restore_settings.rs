//! docs/specs/restore-index-settings: `index_settings` over a stock
//! repository, checked before the first write.

use oss_blob::{BlobStore, S3Op};

use crate::snapshot_restore::send;
use crate::stock_restore::{POOL, RESTORE, register, repository, restored_uuid_missing, rig};

#[test]
fn a_stock_restore_applies_index_settings() {
    let (root, repo) = repository("settings");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let body = r#"{"index_settings":{"index.refresh_interval":"9s"}}"#;
    let reply = send(&r, "POST", RESTORE, Some(body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    let mut buf = vec![0u8; 1 << 16];
    let found = r.descriptors.get("logs", &mut buf).unwrap().unwrap();
    assert!(String::from_utf8_lossy(found.settings).contains("9s"));
}

#[test]
fn a_stock_restore_refuses_a_bad_setting_before_writing_anything() {
    let (root, repo) = repository("badsetting");
    let r = rig(&root, true, POOL);
    register(&r, &repo);
    let puts = r.store.meter().count(S3Op::Put);
    let body = r#"{"index_settings":{"index.number_of_shards":"3"}}"#;
    assert_eq!(send(&r, "POST", RESTORE, Some(body)).status, 400);
    assert_eq!(r.store.meter().count(S3Op::Put), puts);
    assert!(restored_uuid_missing(&r, "logs"));
}
