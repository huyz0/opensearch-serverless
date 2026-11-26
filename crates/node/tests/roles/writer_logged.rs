//! docs/specs/logged-commits: what the writer role logs of a staged commit,
//! and when the log lets it go.

use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_format::ShardKey;
use oss_writer::log_commit::commit_key;

use super::writer_takeover::{Cluster, Node, cluster, create, node_with, seq, write};

const LONG: Duration = Duration::from_secs(3_600);
const WINDOW: Duration = Duration::from_millis(10);

fn commit_logged(n: &Node) -> bool {
    n.committer
        .last_position(&commit_key(&ShardKey::new("u1", 0, None::<&str>)))
        .is_some()
}

fn puts(c: &Cluster) -> u64 {
    c.store.meter().count(S3Op::Put)
}

/// node-1 staging commits with logging on or off: `a` is in the register, `b` staged.
fn staged(c: &Cluster, age: Duration, logging: bool) -> Node {
    create(c, "logs", "u1");
    let one = node_with(c, "node-1", 1, false, None, Some(age));
    if logging {
        assert!(one.role.log_commits(WINDOW));
    }
    assert_eq!(seq(&write(&one.role, "a")), 0);
    assert_eq!(one.role.publish_due().published, 1, "the fence writes");
    assert_eq!(seq(&write(&one.role, "b")), 1);
    assert_eq!(one.role.publish_due().published, 1, "b is staged");
    one
}

// AC-1, AC-3
#[test]
fn a_staged_commit_is_logged_and_released_when_the_register_is_written() {
    let c = cluster();
    let one = staged(&c, Duration::from_millis(80), true);
    std::thread::sleep(Duration::from_millis(40));
    one.role.publish_due();
    assert!(commit_logged(&one), "the staged commit rides the log");
    std::thread::sleep(Duration::from_millis(80));
    let pass = one.role.publish_due();
    assert!(pass.failed.is_empty(), "{:?}", pass.failed);
    assert!(!commit_logged(&one), "the register covers it");
}

// AC-6
#[test]
fn with_logging_off_nothing_is_logged_and_no_request_is_added() {
    let c = cluster();
    let off = staged(&c, LONG, false);
    std::thread::sleep(Duration::from_millis(40));
    let before = puts(&c);
    off.role.publish_due();
    assert!(!commit_logged(&off));
    assert_eq!(puts(&c), before, "no write of its own");
}

#[test]
fn a_shard_let_go_holds_no_logged_commit() {
    let c = cluster();
    let one = staged(&c, LONG, true);
    std::thread::sleep(Duration::from_millis(40));
    one.role.publish_due();
    assert!(commit_logged(&one));
    // Retiring publishes in full, covering the commit.
    one.role.retire(0, u64::MAX);
    assert!(!commit_logged(&one));
}

// AC-2
#[test]
fn with_logging_on_a_staged_commit_costs_at_most_the_one_write_a_tick_makes() {
    let c = cluster();
    let one = staged(&c, LONG, true);
    let before = puts(&c);
    std::thread::sleep(Duration::from_millis(60));
    one.role.publish_due();
    one.role.publish_due();
    assert!(commit_logged(&one));
    assert_eq!(
        puts(&c) - before,
        1,
        "one write for the commit, no register write"
    );
}
