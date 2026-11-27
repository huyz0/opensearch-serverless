//! A staged commit logged under its commit key (docs/specs/logged-commits).

use crate::common;
use std::time::Duration;

use common::committer::{header_at, puts, rec, rig, shard, window};
use common::ids;
use oss_format::ShardKey;
use oss_writer::log::Position;
use oss_writer::log_commit::{CommitRecord, LoggedCommits, commit_key, is_commit_key, shard_uuid};

fn commit(manifest: &str, covered: Option<Position>) -> CommitRecord {
    CommitRecord {
        oldest_pack_millis: 1_000,
        covered,
        manifest: manifest.to_owned(),
    }
}

fn logged(r: &common::committer::Rig, ms: u64) -> LoggedCommits {
    LoggedCommits::new(r.committer.clone(), Duration::from_millis(ms))
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// AC-1
#[test]
fn a_staged_commit_reads_back_with_its_pack_time_and_position() {
    let r = rig(window(20));
    let lc = logged(&r, 20);
    let covered = Some(Position {
        epoch: 7,
        ordinal: 3,
    });
    let sent = commit("{\"files\":[1]}", covered);
    lc.staged(&shard(1), 1, &sent, 1).expect("stages");
    pause(60);
    lc.tick();
    let key = commit_key(&shard(1));
    let at = r.committer.last_position(&key).expect("logged");
    let h = header_at(&r, at);
    let slice = h.slices[0].clone();
    let bytes = r.log.read_slice(at.epoch, &h, &slice).expect("reads");
    assert_eq!(ids(&bytes), ["commit"]);
    let rec = oss_format::slice_records(&bytes)
        .next()
        .expect("one")
        .expect("parses");
    assert_eq!(CommitRecord::parse(rec.source()), Some(sent));
}

#[test]
fn a_commit_with_no_covered_position_round_trips() {
    let sent = commit("m", None);
    assert!(sent.encode().starts_with("1000 -1 -1\n"));
    assert_eq!(CommitRecord::parse(&sent.encode()), Some(sent));
    assert_eq!(CommitRecord::parse("garbage"), None);
    assert_eq!(CommitRecord::parse("1 2\nm"), None);
    assert_eq!(CommitRecord::parse("1 -1 4\nm"), None);
}

// AC-2
#[test]
fn a_logging_node_makes_no_write_for_a_staged_commit() {
    let r = rig(window(80));
    let lc = logged(&r, 80);
    lc.staged(&shard(1), 1, &commit("m", None), 1)
        .expect("stages");
    assert_eq!(puts(&r), 0, "nothing written at staging");
    r.committer
        .commit(&shard(2), 1, &[rec("a", 1)])
        .expect("lands");
    assert_eq!(puts(&r), 1, "the commit rode the record's window");
    pause(200);
    lc.tick();
    assert_eq!(puts(&r), 1, "nothing left for a tick to write");
    assert!(r.committer.last_position(&commit_key(&shard(1))).is_some());
}

// AC-2
#[test]
fn a_quiet_node_writes_once_after_two_windows_for_every_shard_waiting() {
    let r = rig(window(100));
    let lc = logged(&r, 100);
    for n in 0..3 {
        lc.staged(&shard(n), 1, &commit("m", None), 1)
            .expect("stages");
    }
    lc.tick();
    assert_eq!(puts(&r), 0, "not before two windows");
    pause(260);
    lc.tick();
    assert_eq!(puts(&r), 1, "one write for three shards");
    lc.tick();
    assert_eq!(puts(&r), 1, "and none after");
    for n in 0..3 {
        assert!(r.committer.last_position(&commit_key(&shard(n))).is_some());
    }
}

fn runs_held(r: &common::committer::Rig, s: &ShardKey) -> usize {
    r.committer
        .unpublished_runs(&commit_key(s))
        .values()
        .flatten()
        .map(|run| {
            usize::try_from(run.to.saturating_sub(run.from).saturating_add(1)).expect("fits")
        })
        .sum()
}

// AC-3
#[test]
fn only_the_newest_commit_stays_held_until_the_register_covers_it() {
    let r = rig(window(10));
    let lc = logged(&r, 10);
    let s = shard(1);
    for g in 1..=3 {
        lc.staged(&s, 1, &commit(&format!("m{g}"), None), g)
            .expect("stages");
        pause(40);
        lc.tick();
        assert_eq!(runs_held(&r, &s), 1, "after commit {g}");
    }
    lc.checkpointed(&s, 3);
    assert_eq!(runs_held(&r, &s), 0, "the register covers the newest");
    assert!(r.committer.last_position(&commit_key(&s)).is_none());
}

// AC-3
#[test]
fn a_checkpoint_before_the_newest_keeps_the_newest() {
    let r = rig(window(10));
    let lc = logged(&r, 10);
    let s = shard(1);
    lc.staged(&s, 1, &commit("m1", None), 1).expect("stages");
    pause(40);
    lc.tick();
    lc.staged(&s, 1, &commit("m2", None), 2).expect("stages");
    pause(40);
    lc.tick();
    lc.checkpointed(&s, 1);
    assert_eq!(runs_held(&r, &s), 1, "generation 2 is not covered");
    lc.forget(&s);
    assert_eq!(runs_held(&r, &s), 0, "forgotten");
}

// AC-4
#[test]
fn a_record_older_than_the_register_is_released_as_it_lands() {
    let r = rig(window(10));
    let lc = logged(&r, 10);
    let s = shard(1);
    lc.checkpointed(&s, 5);
    lc.staged(&s, 1, &commit("old", None), 4).expect("stages");
    pause(40);
    lc.tick();
    assert_eq!(runs_held(&r, &s), 0);
}

// AC-5
#[test]
fn a_commit_key_is_not_a_shard() {
    let key = commit_key(&shard(1));
    assert!(is_commit_key(&key));
    assert!(!is_commit_key(&shard(1)));
    assert_eq!(shard_uuid(&key.index_uuid), "uuid-1");
    assert_eq!(shard_uuid("uuid-1"), "uuid-1");
    assert_eq!(key.shard, 1);
}

// AC-5
#[test]
fn a_successor_is_not_told_of_a_commit_key_as_a_shard() {
    let r = rig(window(10));
    let lc = logged(&r, 10);
    r.committer
        .commit(&shard(1), 1, &[rec("a", 1)])
        .expect("lands");
    lc.staged(&shard(2), 1, &commit("m", None), 1)
        .expect("stages");
    pause(40);
    lc.tick();
    assert!(r.committer.last_position(&commit_key(&shard(2))).is_some());
    let takeover =
        oss_writer::log::NodeLogTakeover::new(r.log.clone(), r.committer.positions(), "node-2", 2);
    assert_eq!(
        takeover
            .named()
            .expect("fenced")
            .into_iter()
            .collect::<Vec<_>>(),
        [shard(1)]
    );
    assert_eq!(
        takeover.named_within(None).expect("fenced").len(),
        1,
        "named_within too"
    );
}

// AC-3
#[test]
fn a_commit_still_queued_when_its_shard_is_let_go_is_released_as_it_lands() {
    let r = rig(window(10));
    let lc = logged(&r, 10);
    let s = shard(1);
    lc.staged(&s, 1, &commit("m", None), 1).expect("stages");
    lc.forget(&s);
    pause(40);
    lc.tick();
    assert_eq!(runs_held(&r, &s), 0, "nothing holds the log");
    assert!(r.committer.last_position(&commit_key(&s)).is_none());
    lc.staged(&s, 1, &commit("m2", None), 2).expect("stages");
    pause(40);
    lc.tick();
    assert_eq!(runs_held(&r, &s), 1, "a later one is held again");
}
