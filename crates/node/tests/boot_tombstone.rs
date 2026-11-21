//! docs/specs/node-tombstone-sweep AC-10: a booted node removes the old
//! tombstone of a deleted index on its own.

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, FsStore};
use oss_node::boot::{NodeConfig, start};
use oss_testkit::TempDir;

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .expect("a deadline");
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lookup(
    extra: &'static [(&'static str, &'static str)],
    root: String,
) -> impl Fn(&str) -> Option<String> {
    move |key| {
        extra
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
            .or_else(|| match key {
                "OSS_NODE_ID" => Some("n1".to_owned()),
                "OSS_STORE" => Some("fs".to_owned()),
                "OSS_FS_ROOT" => Some(root.clone()),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
                _ => None,
            })
    }
}

/// As `config_with`, sweeping tombstones every millisecond.
fn config_sweep(root: &std::path::Path, id: &str) -> NodeConfig {
    let root = root.display().to_string();
    let lookup = |key: &str| {
        Some(
            match key {
                "OSS_NODE_ID" => id,
                "OSS_STORE" => "fs",
                "OSS_FS_ROOT" => root.as_str(),
                "OSS_HTTP_ADDR" => "127.0.0.1:0",
                "OSS_TRANSPORT_ADDR" => "127.0.0.1:0",
                "OSS_LEASE_TTL_MS" => "3000",
                "OSS_JANITOR_EVERY_MS" => "50",
                "OSS_TOMBSTONE_SWEEP_MS" => "1",
                _ => return None,
            }
            .to_owned(),
        )
    };
    NodeConfig::from_lookup(lookup).expect("valid")
}

// docs/specs/node-tombstone-sweep AC-10
#[test]
fn a_booted_node_removes_the_old_tombstone_of_a_deleted_index() {
    let dir = TempDir::new("boot-tombstone");
    let store: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).expect("store"));
    let descriptors =
        oss_meta::DescriptorStore::new(Arc::clone(&store), "", oss_meta::DeleteMode::Tombstone)
            .expect("descriptors");
    // Deleted in 2020: long past the quarantine.
    let at = 1_600_000_000_000;
    let mut buf = vec![0u8; 4096];
    let g = descriptors
        .create(&oss_format::IndexRecord::new("gone", "u", 1), at, &mut buf)
        .expect("created");
    assert!(
        descriptors
            .delete_if_unchanged("gone", g, at)
            .expect("deleted")
    );
    let markers = || {
        let mut n = 0;
        store
            // LIST: a test counting the markers left.
            .list_prefix("tombstones/", None, 1000, &mut |_, _| n += 1)
            .expect("lists");
        n
    };
    assert_eq!(markers(), 1);
    let node = start(&config_sweep(dir.path(), "n1")).expect("starts");
    wait_for("the tombstone was never swept", || markers() == 0);
    drop(node);
}

#[test]
fn the_tombstone_sweep_is_hourly_unless_set_and_zero_turns_it_off() {
    let config = |extra| NodeConfig::from_lookup(lookup(extra, "/x".to_owned())).expect("valid");
    assert_eq!(config(&[]).tombstone_sweep_millis, 3_600_000);
    assert_eq!(
        config(&[("OSS_TOMBSTONE_SWEEP_MS", "0")]).tombstone_sweep_millis,
        0
    );
    assert!(
        NodeConfig::from_lookup(lookup(&[("OSS_TOMBSTONE_SWEEP_MS", "x")], "/x".to_owned()))
            .is_err()
    );
}
