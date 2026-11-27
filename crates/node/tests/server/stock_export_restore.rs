//! docs/specs/stock-snapshot-export AC-9: both snapshots of a repository
//! the Rust node wrote restore on the Rust node.

#![allow(clippy::disallowed_methods)]

use oss_format::ShardRef;

use crate::stock_export::{TAKE, WAIT, rig, send, shard_files};

/// Both snapshots of a repository the Rust node wrote restore on the Rust
/// node, each into its own new index, every file whole.
#[test]
fn both_snapshots_of_a_repository_restore_on_the_rust_node() {
    use crate::snapshot_restore::{read, restored_uuid, send as send_restore};
    use crate::stock_restore::{POOL, register, rig as restore_rig};

    let r = rig(true, "restore-both");
    for name in ["s1", "s2"] {
        let reply = send(
            &r,
            "PUT",
            &format!("/_snapshot/stock/{name}{WAIT}"),
            Some(TAKE),
        );
        assert_eq!(reply.status, 200, "{}", reply.text());
    }
    let node = restore_rig(&r.root, true, POOL);
    register(&node, &r.out());
    for (snap, index) in [("s1", "logs_one"), ("s2", "logs_two")] {
        let body = format!(r#"{{"rename_pattern":"logs","rename_replacement":"{index}"}}"#);
        let reply = send_restore(
            &node,
            "POST",
            &format!("/_snapshot/stock/{snap}/_restore{WAIT}"),
            Some(&body),
        );
        assert_eq!(reply.status, 200, "{snap}: {}", reply.text());
        let uuid = restored_uuid(&node, index);
        let shard = ShardRef {
            index,
            uuid: &uuid,
            shard: 0,
        };
        let dir = format!("{}t=1/", shard.base_path("").unwrap());
        for (file, bytes) in shard_files() {
            assert_eq!(read(&node, &format!("{dir}{file}")), bytes, "{snap} {file}");
        }
    }
}
