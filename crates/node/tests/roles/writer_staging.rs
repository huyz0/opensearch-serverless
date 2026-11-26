//! docs/specs/commit-staging: what the writer role does with a staged commit:
//! the log is trimmed and a shard is called published only behind a register
//! write, and a node lost with staged commits loses nothing acknowledged.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_format::{CommitManifest, ShardKey, ShardRef};
use oss_writer::log::Position;

use super::writer_takeover::{
    Cluster, Node, cluster, create, info, node_with, registered_term, seq, takeover, write,
};

const LONG: Duration = Duration::from_secs(3_600);

fn key() -> ShardKey {
    ShardKey::new("u1", 0, None::<&str>)
}

fn logged(n: &Node) -> Option<Position> {
    n.committer.last_position(&key())
}

fn register(c: &Cluster) -> (u64, CommitManifest<'static>) {
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
    let bytes: &'static [u8] = Box::leak(buf[..read.len].to_vec().into_boxed_slice());
    (
        read.generation,
        CommitManifest::parse(bytes).expect("parses"),
    )
}

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

/// node-1 staging commits: `a` written to the register, `b` and `c` staged.
fn staging_node(c: &Cluster, age: Duration) -> Node {
    create(c, "logs", "u1");
    let one = node_with(c, "node-1", 1, false, None, Some(age));
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(one.role.publish_due().published, 1, "the fence writes");
    assert!(logged(&one).is_none(), "a register write trims the log");
    one
}

#[test]
fn a_staged_publish_trims_nothing_and_leaves_the_shard_unpublished_until_the_register_is_written() {
    let c = cluster();
    let one = staging_node(&c, LONG);
    let (generation, _) = register(&c);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert!(logged(&one).is_some());
    let before = puts(&c);
    assert_eq!(one.role.publish_due().published, 1);
    assert_eq!(puts(&c) - before, 1, "a blob, no swap");
    assert_eq!(register(&c).0, generation, "the register did not move");
    assert!(
        logged(&one).is_some(),
        "the log is kept behind the register"
    );
    // Asleep, the shard is published in full first: the register moves and
    // the log is trimmed.
    assert_eq!(one.role.hibernate_idle(0, 10_000).hibernated, 1);
    let (after, manifest) = register(&c);
    assert!(after > generation, "the register was written");
    assert!(manifest.file("segments_2").is_some() || manifest.files().len() > 1);
    assert!(logged(&one).is_none());
}

#[test]
fn a_checkpoint_writes_the_register_and_trims_the_log() {
    let c = cluster();
    let one = staging_node(&c, Duration::from_millis(50));
    let (generation, _) = register(&c);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert_eq!(one.role.publish_due().published, 1);
    assert_eq!(register(&c).0, generation);
    assert!(logged(&one).is_some());
    std::thread::sleep(Duration::from_millis(80));
    let before = puts(&c);
    let pass = one.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    assert_eq!(puts(&c) - before, 1, "one swap, no snapshot, no upload");
    assert!(register(&c).0 > generation);
    assert!(logged(&one).is_none());
}

#[test]
fn a_node_lost_with_staged_commits_loses_no_acknowledged_write() {
    let c = cluster();
    let one = staging_node(&c, LONG);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert_eq!(one.role.publish_due().published, 1, "b is staged");
    assert_eq!(seq(&write(&one.role, "c")), 2);
    assert_eq!(registered_term(&c), 1);
    // node-1 is lost: its staged commit with it. The successor restores the
    // register's commit and replays the log, which was never trimmed.
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node_with(
        &c,
        "node-2",
        2,
        true,
        Some(takeover(&c, &one, &calls)),
        None,
    );
    assert_eq!(seq(&write(&two.role, "d")), 3, "a, b and c were all kept");
    assert_eq!(
        registered_term(&c),
        2,
        "b and c were published under the new term"
    );
    let _ = (&c.store as &Arc<_>, info());
}

#[test]
fn a_node_told_to_stop_writes_the_register_of_a_shard_with_a_staged_commit() {
    let c = cluster();
    let one = staging_node(&c, LONG);
    let (generation, _) = register(&c);
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert_eq!(one.role.publish_due().published, 1, "b is staged");
    assert_eq!(register(&c).0, generation);
    one.role.retire(0, u64::MAX);
    assert!(register(&c).0 > generation, "retiring publishes in full");
    assert!(logged(&one).is_none());
}
