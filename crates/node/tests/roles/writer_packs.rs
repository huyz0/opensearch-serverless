//! docs/specs/node-pack-wiring: a node told to pack writes its small segments
//! into packs, and one told nothing writes blobs as before.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_format::ShardRef;
use oss_node::writer::PackConfig;

use super::writer_takeover::{
    Cluster, Node, cluster, create, node_full, registered_term, seq, takeover, write,
};

const LEDGER: &str = "packs/node-1/ledger";

fn packs() -> PackConfig {
    PackConfig {
        max_blob_bytes: 8 << 20,
        linger: Duration::from_millis(1),
    }
}

fn node(c: &Cluster, packing: bool) -> Node {
    create(c, "logs", "u1");
    node_full(
        c,
        "node-1",
        1,
        false,
        None,
        None,
        (false, false, None),
        packing.then(packs),
    )
}

fn register_bytes(c: &Cluster) -> Vec<u8> {
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&at.manifest_key("").expect("key"), &mut buf)
        .expect("reads")
        .expect("registered");
    buf[..read.len].to_vec()
}

fn names_a_pack(c: &Cluster) -> bool {
    let bytes = register_bytes(c);
    String::from_utf8_lossy(&bytes).contains("@node-1/e=")
}

fn has_ledger(c: &Cluster) -> bool {
    let mut buf = vec![0u8; 1 << 16];
    c.store
        .read_register(LEDGER, &mut buf)
        .expect("reads")
        .is_some()
}

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

fn publish(n: &Node, id: &str) {
    write(&n.role, id);
    let pass = n.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    assert_eq!(pass.published, 1);
}

// AC-3
#[test]
fn with_packs_off_a_publish_writes_blobs_and_no_pack_or_ledger() {
    let c = cluster();
    let n = node(&c, false);
    publish(&n, "a");
    assert!(!names_a_pack(&c));
    assert!(!has_ledger(&c));
}

// AC-4
#[test]
fn with_packs_on_a_publish_is_one_pack_put_and_names_the_pack() {
    let c = cluster();
    let off = {
        let c = cluster();
        let n = node(&c, false);
        write(&n.role, "a");
        let before = puts(&c);
        n.role.publish_due();
        puts(&c) - before
    };
    let n = node(&c, true);
    write(&n.role, "a");
    let before = puts(&c);
    let pass = n.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    let on = puts(&c) - before;
    assert!(names_a_pack(&c), "the manifest names a pack location");
    assert!(has_ledger(&c), "the epoch is recorded");
    // Packs on add the ledger create to the one pack PUT; the segment's own
    // blob PUTs are gone, so a segment of several files is no more PUTs.
    assert!(on <= off + 1, "packs on {on}, off {off}");
}

// AC-5
#[test]
fn a_node_taking_over_a_packed_shard_reads_every_document() {
    let c = cluster();
    let one = node(&c, true);
    publish(&one, "a");
    assert!(names_a_pack(&c));
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node_full(
        &c,
        "node-2",
        2,
        true,
        Some(takeover(&c, &one, &calls)),
        None,
        (false, false, None),
        None,
    );
    assert_eq!(
        seq(&write(&two.role, "b")),
        1,
        "a was restored from its pack"
    );
    assert_eq!(two.role.publish_due().published, 1);
    assert_eq!(registered_term(&c), 2);
}

// AC-6
#[test]
fn a_node_that_publishes_nothing_makes_no_pack_request() {
    let c = cluster();
    let n = node(&c, true);
    let before = puts(&c);
    assert_eq!(n.role.publish_due().published, 0);
    assert_eq!(puts(&c), before);
    assert!(!has_ledger(&c));
}

// AC-7
#[test]
fn a_later_publish_reuses_the_writer_and_writes_the_ledger_once() {
    let c = cluster();
    let n = node(&c, true);
    publish(&n, "a");
    let mut buf = vec![0u8; 1 << 16];
    let first = c
        .store
        .read_register(LEDGER, &mut buf)
        .expect("reads")
        .expect("ledger")
        .generation;
    publish(&n, "b");
    let second = c
        .store
        .read_register(LEDGER, &mut buf)
        .expect("reads")
        .expect("ledger")
        .generation;
    assert_eq!(first, second, "the ledger was not written again");
}
