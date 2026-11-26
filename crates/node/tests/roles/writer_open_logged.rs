//! docs/specs/open-from-logged-commit: a successor opens a taken shard from
//! the newest commit its predecessor logged, and writes it to the register.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_format::{CommitManifest, FileLocation, ShardRef};
use oss_writer::bulk::ItemResult;

use super::writer_takeover::{
    Cluster, Node, cluster, create, node_logging, registered_term, seq, takeover, write,
};

const LONG: Duration = Duration::from_secs(3_600);

fn register(c: &Cluster) -> CommitManifest<'static> {
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
    CommitManifest::parse(bytes).expect("parses")
}

/// node-1 staging and logging: `a` in the register, `b` staged and logged,
/// and with `tail` `c` acknowledged and only in the log.
fn lost_predecessor(c: &Cluster, tail: bool) -> Node {
    create(c, "logs", "u1");
    let one = node_logging(c, "node-1", 1, false, None, Some(LONG), (true, false));
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(one.role.publish_due().published, 1, "the fence writes");
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert_eq!(one.role.publish_due().published, 1, "b is staged");
    // The tick that writes what no other record carried.
    std::thread::sleep(Duration::from_millis(60));
    one.role.publish_due();
    if tail {
        assert_eq!(seq(&write(&one.role, "c")), 2);
    }
    one
}

/// node-2 taking the shard over: the PUTs and GETs it made, and the term of the
/// register after.
fn taken_over(opening: bool, tail: bool) -> (u64, Vec<String>) {
    let c = cluster();
    let one = lost_predecessor(&c, tail);
    let calls = Arc::new(AtomicUsize::new(0));
    let lists = c.store.meter().count(S3Op::List);
    let two = node_logging(
        &c,
        "node-2",
        2,
        true,
        Some(takeover(&c, &one, &calls)),
        None,
        (false, opening),
    );
    assert_eq!(
        seq(&write(&two.role, "d")),
        if tail { 3 } else { 2 },
        "everything acknowledged was kept"
    );
    two.role.publish_due();
    assert_eq!(c.store.meter().count(S3Op::List), lists, "no LIST");
    // Files the register names from the predecessor's term: what was not
    // replayed and uploaded again.
    let files = register(&c)
        .files()
        .iter()
        .filter(|f| match &f.location {
            FileLocation::Whole { term_dir } | FileLocation::Packed { term_dir, .. } => {
                term_dir.contains('1')
            }
            _ => false,
        })
        .map(|f| f.name.to_string())
        .collect();
    (registered_term(&c), files)
}

// AC-1, AC-6
#[test]
fn a_taken_shard_with_nothing_more_to_replay_is_adopted_by_one_swap() {
    let (plain_term, plain) = taken_over(false, false);
    let (logged_term, logged) = taken_over(true, false);
    assert_eq!((plain_term, logged_term), (2, 2));
    assert!(
        logged.len() > plain.len(),
        "b stayed in the predecessor's blobs: {logged:?} against {plain:?}"
    );
}

// AC-1, AC-6
#[test]
fn a_taken_shard_replays_only_what_the_logged_commit_lacks() {
    let (plain_term, plain) = taken_over(false, true);
    let (logged_term, logged) = taken_over(true, true);
    assert_eq!((plain_term, logged_term), (2, 2));
    assert!(
        logged.len() > plain.len(),
        "b stayed in the predecessor's blobs: {logged:?} against {plain:?}"
    );
}

// AC-4
#[test]
fn with_the_setting_off_the_open_is_what_it_was() {
    let (a, files_a) = taken_over(false, true);
    let (b, files_b) = taken_over(false, true);
    assert_eq!(a, b);
    assert_eq!(files_a, files_b);
}

// AC-2, AC-6: whichever read of the open fails, nothing acknowledged is lost: a
// logged commit that cannot be restored is a hint that failed, and the shard
// opens again from the register.
#[test]
fn a_failed_read_during_the_open_loses_nothing() {
    for failed in 1..=8 {
        let c = cluster();
        let one = lost_predecessor(&c, true);
        let calls = Arc::new(AtomicUsize::new(0));
        let two = node_logging(
            &c,
            "node-2",
            2,
            true,
            Some(takeover(&c, &one, &calls)),
            None,
            (false, true),
        );
        c.store.fail_next(S3Op::GetRange, failed);
        // The first write after a failure may be refused; a later one is not.
        let mut written = None;
        for _ in 0..4 {
            let done = write(&two.role, "d");
            if let ItemResult::Written { written: w, .. } = &done {
                written = Some(w.seq_no);
                break;
            }
        }
        assert_eq!(
            written,
            Some(3),
            "a, b and c were all kept (failed {failed})"
        );
    }
}
