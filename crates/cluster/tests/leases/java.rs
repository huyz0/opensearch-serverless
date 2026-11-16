//! A lease and members index the Java wrote, in registers on the
//! filesystem store, read live by a Rust refresh (AC-10). That the Java
//! reads what Rust writes follows from AC-1: Rust writes the Java's own
//! bytes, which the recorder checked the Java reads back.

#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, FsStore};
use oss_cluster::{LeaseClock, Membership};
use oss_testkit::TempDir;

use crate::{BASE, Fleet, ids};

fn recorded(name: &str, line: usize) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/node-leases")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let line = text.lines().nth(line).unwrap();
    if name.ends_with("indexes.txt") {
        (0..line.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
            .collect()
    } else {
        line.as_bytes().to_vec()
    }
}

#[test]
fn a_java_lease_on_fs_is_read_live() {
    let dir = TempDir::new("java-lease");
    let store: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()).unwrap());
    // The Java's lease of n1 (expiring at T0 + 30 s) and the index naming it.
    store
        .create_register(
            &format!("{BASE}cluster/members/lease-n1"),
            &recorded("parsed.txt", 0),
        )
        .unwrap();
    store
        .create_register(
            &format!("{BASE}cluster/members/members"),
            &recorded("indexes.txt", 1),
        )
        .unwrap();
    let fleet = Fleet::new();
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    let node = Membership::new(Arc::clone(&store), BASE, clock, fleet.config(false))
        .unwrap()
        .with_probe_interval(Duration::ZERO);
    let seen = node.refresh().unwrap();
    assert_eq!(ids(&seen.live), ["n1"]);
    assert_eq!(seen.live[0].address, "10.0.0.1:9300");
    // A Rust node joins beside it: the index then names both, in the
    // Java's order.
    node.renew(&crate::lease("n0")).unwrap();
    assert_eq!(node.enrolled_ids().unwrap(), ["n0", "n1"]);
    fleet.clock.advance(30_000);
    node.renew(&crate::lease("n0")).unwrap();
    assert_eq!(ids(&node.refresh().unwrap().live), ["n0"], "n1 expired");
}
