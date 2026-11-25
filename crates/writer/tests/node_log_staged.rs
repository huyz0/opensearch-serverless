//! The two halves of a commit (docs/specs/write-pipeline): records staged
//! one after another share a window, and a staged window is driven to its
//! end without being waited for.

mod common;

use std::sync::Arc;

use common::committer::{puts, rec, rig, shard, window};
use common::ids;
use oss_blob::S3Op;
use oss_writer::log::CommitError;

// AC-1
#[test]
fn records_staged_one_after_another_share_a_window_and_a_put() {
    let r = rig(window(300));
    let first = r
        .committer
        .stage(&shard(1), 1, &[rec("a", 1)])
        .expect("stages")
        .expect("some");
    let second = r
        .committer
        .stage(&shard(1), 1, &[rec("b", 2)])
        .expect("stages")
        .expect("some");
    let c = Arc::clone(&r.committer);
    let waiter = std::thread::spawn(move || second.wait(&c));
    let at = first.wait(&r.committer).expect("lands").expect("some");
    assert_eq!(waiter.join().expect("ends").expect("lands"), Some(at));
    assert_eq!(puts(&r), 1, "one window, one PUT");
    let h = common::committer::header_at(&r, at);
    let slice = h.slices[0].clone();
    assert_eq!(
        ids(&r.log.read_slice(7, &h, &slice).expect("reads")),
        ["a", "b"],
        "in the order staged"
    );
}

// AC-1
#[test]
fn staging_no_records_stages_nothing() {
    let r = rig(window(0));
    assert!(r.committer.stage(&shard(1), 1, &[]).expect("ok").is_none());
    assert_eq!(puts(&r), 0);
}

// AC-2
#[test]
fn finishing_a_staged_window_writes_it_without_a_wait() {
    let r = rig(window(0));
    let staged = r
        .committer
        .stage(&shard(1), 1, &[rec("a", 1)])
        .expect("stages")
        .expect("some");
    assert!(!staged.finish(&r.committer), "no panic");
    assert_eq!(puts(&r), 1, "the window is written");
    assert!(staged.wait(&r.committer).expect("lands").is_some());
    assert_eq!(puts(&r), 1, "waiting after finishing writes nothing more");
}

// AC-2
#[test]
fn finishing_a_failed_window_reports_the_failure_to_the_wait() {
    let r = rig(window(0));
    r.store.fail_next(S3Op::Put, 1);
    let staged = r
        .committer
        .stage(&shard(1), 1, &[rec("a", 1)])
        .expect("stages")
        .expect("some");
    assert!(
        !staged.finish(&r.committer),
        "a failed write is not a panic"
    );
    let failed = staged.wait(&r.committer);
    assert!(matches!(failed, Err(CommitError::Store(_))), "{failed:?}");
}
