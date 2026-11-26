//! docs/specs/node-pack-wiring: a node told to pack writes its small segments
//! into packs, and one told nothing writes blobs as before.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_format::ShardRef;
use oss_node::writer::PackConfig;

use super::writer_takeover::{
    Cluster, Node, cluster, create, node_full, registered_term, seq, takeover, write, write_to,
};

const LEDGER: &str = "packs/node-1/ledger";

fn packs() -> PackConfig {
    PackConfig {
        max_blob_bytes: 8 << 20,
        linger: Duration::from_millis(1),
        publish_threads: 8,
        collect: false,
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

fn register_bytes(c: &Cluster) -> Option<Vec<u8>> {
    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&at.manifest_key("").expect("key"), &mut buf)
        .expect("reads")?;
    Some(buf[..read.len].to_vec())
}

fn names_a_pack(c: &Cluster) -> bool {
    let bytes = register_bytes(c).unwrap_or_default();
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
    // Off: the segment's blob and the register swap. On: one pack, the
    // ledger's first create and the swap; no blob of its own.
    assert_eq!((off, on), (2, 3));
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

// AC-8
#[test]
fn a_ledger_that_cannot_be_read_fails_the_publish_and_the_next_pass_packs() {
    let c = cluster();
    let n = node(&c, true);
    c.store
        .create_register(LEDGER, b"not a ledger")
        .expect("planted");
    write(&n.role, "a");
    let pass = n.role.publish_due();
    assert_eq!(pass.published, 0);
    assert_eq!(pass.failed.len(), 1, "{:?}", pass.failed);
    assert!(!names_a_pack(&c), "no silent fallback to blobs");
    let mut buf = vec![0u8; 1 << 16];
    let planted = c
        .store
        .read_register(LEDGER, &mut buf)
        .expect("reads")
        .expect("planted");
    assert!(
        c.store
            .delete_register(LEDGER, planted.generation)
            .expect("removed")
    );
    let pass = n.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    assert_eq!(pass.published, 1, "the shard stayed due");
    assert!(names_a_pack(&c));
}

fn exists(c: &Cluster, key: &str) -> bool {
    c.store.head(key).expect("head").is_some()
}

const FIRST_PACK: &str = "packs/node-1/e=00000000000000001000/00000000000000000000";
const SECOND_PACK: &str = "packs/node-1/e=00000000000000001000/00000000000000000001";

/// node-1 packing with a long linger and `threads`, two shards each holding a write.
fn two_shards(c: &Cluster, threads: usize) -> Node {
    create(c, "logs", "u1");
    create(c, "logs2", "u2");
    let n = node_full(
        c,
        "node-1",
        1,
        false,
        None,
        None,
        (false, false, None),
        Some(PackConfig {
            max_blob_bytes: 8 << 20,
            linger: Duration::from_millis(1_000),
            publish_threads: threads,
            collect: false,
        }),
    );
    write(&n.role, "a");
    write_to(&n.role, "logs2", "b");
    n
}

// AC-2
#[test]
fn two_due_shards_of_one_pass_share_one_pack() {
    let c = cluster();
    let n = two_shards(&c, 8);
    let before = puts(&c);
    let pass = n.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    assert_eq!(pass.published, 2);
    for (index, uuid) in [("logs", "u1"), ("logs2", "u2")] {
        let at = ShardRef {
            index,
            uuid,
            shard: 0,
        };
        let mut buf = vec![0u8; 1 << 16];
        let read = c
            .store
            .read_register(&at.manifest_key("").expect("key"), &mut buf)
            .expect("reads")
            .expect("registered");
        let manifest = String::from_utf8_lossy(&buf[..read.len]).into_owned();
        assert!(
            manifest.contains("@node-1/e=00000000000000001000/00000000000000000000#"),
            "{uuid} names the shared pack: {manifest}"
        );
    }
    assert!(exists(&c, FIRST_PACK), "one pack");
    assert!(!exists(&c, SECOND_PACK), "and no second");
    // One pack, the ledger's first create, and a swap a shard.
    assert_eq!(puts(&c) - before, 4);
}

// AC-3
#[test]
fn with_one_thread_each_shard_is_a_pack_of_its_own() {
    let c = cluster();
    let n = two_shards(&c, 1);
    let pass = n.role.publish_due();
    assert_eq!(pass.published, 2, "{:?}", pass.failed);
    assert!(exists(&c, FIRST_PACK) && exists(&c, SECOND_PACK));
}

// AC-5, AC-6
#[test]
fn a_failing_shard_is_reported_and_the_others_are_published() {
    let c = cluster();
    let n = two_shards(&c, 8);
    // A shard register no publisher can read.
    let at = ShardRef {
        index: "logs2",
        uuid: "u2",
        shard: 0,
    };
    c.store
        .create_register(&at.manifest_key("").expect("key"), b"not a manifest")
        .expect("planted");
    let pass = n.role.publish_due();
    assert_eq!(pass.published, 1, "{:?}", pass.failed);
    assert_eq!(pass.failed.len(), 1, "{:?}", pass.failed);
    assert!(pass.failed[0].contains("[u2][0]"), "{:?}", pass.failed);
}

// AC-1, AC-2: with collection on a publish is still pack, ledger create and
// swap, and the ledger already reserves its high a block ahead.
#[test]
fn with_collection_on_a_first_publish_costs_the_same_and_reserves_ahead() {
    let c = cluster();
    create(&c, "logs", "u1");
    let n = node_full(
        &c,
        "node-1",
        1,
        false,
        None,
        None,
        (false, false, None),
        Some(PackConfig {
            collect: true,
            ..packs()
        }),
    );
    write(&n.role, "a");
    let before = c.store.meter().count(S3Op::Put);
    let pass = n.role.publish_due();
    assert_eq!(pass.published, 1, "{:?}", pass.failed);
    assert_eq!(
        c.store.meter().count(S3Op::Put) - before,
        3,
        "pack, ledger, swap"
    );
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(LEDGER, &mut buf)
        .expect("reads")
        .expect("ledger");
    let parsed = oss_format::NodePackLedger::parse(&buf[..read.len]).expect("v1");
    assert_eq!(parsed.epochs().len(), 1);
    assert_eq!(parsed.epochs()[0].high, 8, "a block ahead");
}
