//! docs/specs/janitor-published/SPEC.md: which shards a pass sweeps, on the
//! emulator with its meter.

// ARITH: times and counts here are test constants and small sums.
#![allow(clippy::arithmetic_side_effects)]

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource, SwapOutcome};
use oss_format::{FileLocation, ManifestEntry, ShardRef, encode_manifest};
use oss_janitor::JanitorError;
use oss_janitor::gc::{Collector, Config, Outcome, Pinned, Pins};
use oss_janitor::published::{OwnedShard, Published, Sweeps};

const BASE: &str = "d/";
const A: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 0,
};
const B: ShardRef<'static> = ShardRef {
    index: "logs",
    uuid: "u1",
    shard: 1,
};
const LIVE: Config = Config {
    dry_run: false,
    minimum_unreferenced_millis: 0,
    max_watched: 100_000,
    max_pinned: 100_000,
    relist_after_millis: 0,
};

struct NoPins;

impl Pins for NoPins {
    fn pinned(&self, _: &ShardRef<'_>) -> Result<Pinned, JanitorError> {
        Ok(Pinned::Blobs(Vec::new()))
    }
}

struct Rig {
    store: Emulator,
    sweeps: Sweeps,
}

fn path(shard: &ShardRef<'_>) -> String {
    format!("d/segments/{}#{}#{}/", shard.index, shard.uuid, shard.shard)
}

impl Rig {
    fn new() -> Self {
        Self {
            store: Emulator::new(64 << 20, 1),
            sweeps: Sweeps::new(BASE),
        }
    }

    fn blob(&self, shard: &ShardRef<'_>, rest: &str) {
        let mut buf = [0u8; 64];
        self.store
            .put(
                &format!("{}{rest}", path(shard)),
                &mut SliceSource::new(b"bytes"),
                &mut buf,
            )
            .unwrap();
    }

    fn exists(&self, shard: &ShardRef<'_>, rest: &str) -> bool {
        self.store
            .head(&format!("{}{rest}", path(shard)))
            .unwrap()
            .is_some()
    }

    /// Publishes a manifest of `term` naming `files`, creating or replacing
    /// the register.
    fn manifest(&self, shard: &ShardRef<'_>, term: u64, files: &[&str]) {
        let entries: Vec<ManifestEntry<'_>> = files
            .iter()
            .map(|name| ManifestEntry {
                name,
                location: FileLocation::Whole { term_dir: "t=2" }.encode(),
                length: 5,
            })
            .collect();
        let value = encode_manifest(term, Some("n1"), &entries, -1, -1).unwrap();
        let key = format!("{}manifest", path(shard));
        let mut buf = vec![0u8; 1 << 12];
        match self.store.read_register(&key, &mut buf).unwrap() {
            None => assert!(matches!(
                self.store.create_register(&key, &value).unwrap(),
                SwapOutcome::Applied(_)
            )),
            Some(read) => assert!(matches!(
                self.store
                    .swap_register(&key, read.generation, &value)
                    .unwrap(),
                SwapOutcome::Applied(_)
            )),
        }
    }

    fn pass(&mut self, published: &[(ShardRef<'_>, &[&str])]) -> Vec<Reported> {
        let with_term: Vec<_> = published.iter().map(|(s, f)| (*s, 2, *f)).collect();
        self.pass_at(&with_term)
    }

    fn pass_at(&mut self, published: &[(ShardRef<'_>, u64, &[&str])]) -> Vec<Reported> {
        let clock = || 1_000;
        let collector = Collector::new(&self.store, BASE, &NoPins, &clock, LIVE);
        let published: Vec<Published<'_>> = published
            .iter()
            .map(|(shard, term, files)| Published {
                shard: *shard,
                term: *term,
                files,
            })
            .collect();
        let mut buf = vec![0u8; 1 << 16];
        self.sweeps
            .sweep_published(&collector, &published, &mut buf)
            .into_iter()
            .map(|(shard, result)| Reported { shard, result })
            .collect()
    }

    fn requests(&self) -> u64 {
        self.store.meter().total()
    }

    fn lists(&self) -> u64 {
        self.store.meter().count(S3Op::List)
    }
}

struct Reported {
    shard: OwnedShard,
    result: Result<oss_janitor::gc::Sweep, JanitorError>,
}

#[test]
fn a_first_publish_is_swept_and_an_add_only_one_after_it_is_not() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs", "segments_1"]);
    let first = rig.pass(&[(A, &["_0.cfs", "segments_1"])]);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].shard.shard, 0);
    assert_eq!(first[0].result.as_ref().unwrap().outcome, Outcome::Swept);
    assert_eq!(rig.lists(), 1);

    let before = rig.requests();
    let added = rig.pass(&[(A, &["_0.cfs", "_1.cfs", "segments_2"])]);
    assert!(added.is_empty());
    assert_eq!(rig.requests(), before);
}

#[test]
fn a_dropped_file_triggers_a_sweep_and_a_lost_segments_file_does_not() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs", "_1.cfs", "segments_1"]);
    rig.pass(&[(A, &["_0.cfs", "_1.cfs", "segments_1"])]);

    let before = rig.requests();
    assert!(
        rig.pass(&[(A, &["_0.cfs", "_1.cfs", "segments_2"])])
            .is_empty()
    );
    assert_eq!(rig.requests(), before);

    let merged = rig.pass(&[(A, &["_2.cfs", "segments_3"])]);
    assert_eq!(merged.len(), 1);
    assert!(rig.requests() > before);
}

#[test]
fn a_watched_shard_is_swept_without_being_published_until_nothing_is_watched() {
    let mut rig = Rig::new();
    rig.blob(&A, "t=1/_0.seg");
    rig.manifest(&A, 2, &["_1.cfs"]);
    let first = rig.pass(&[(A, &["_1.cfs"])]);
    assert_eq!(first[0].result.as_ref().unwrap().watching, 1);
    assert_eq!(rig.sweeps.memory().watching(&path(&A)), 1);

    let second = rig.pass(&[]);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].shard.index, "logs");
    assert_eq!(second[0].result.as_ref().unwrap().deleted.len(), 1);
    assert!(!rig.exists(&A, "t=1/_0.seg"));

    let before = rig.requests();
    assert!(rig.pass(&[]).is_empty());
    assert_eq!(rig.requests(), before);
}

#[test]
fn a_shard_both_published_and_watched_is_swept_once() {
    let mut rig = Rig::new();
    rig.blob(&A, "t=1/_0.seg");
    rig.manifest(&A, 2, &["_1.cfs"]);
    rig.pass(&[(A, &["_1.cfs"])]);
    let lists = rig.lists();
    let again = rig.pass(&[(A, &["_2.cfs"])]);
    assert_eq!(again.len(), 1);
    assert_eq!(rig.lists(), lists + 1);
}

#[test]
fn force_makes_the_next_pass_sweep_the_shard_once() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs"]);
    rig.pass(&[(A, &["_0.cfs"])]);
    rig.sweeps.force(A);
    let forced = rig.pass(&[]);
    assert_eq!(forced.len(), 1);
    assert_eq!(forced[0].shard.as_ref(), A);
    let before = rig.requests();
    assert!(rig.pass(&[]).is_empty());
    assert_eq!(rig.requests(), before);
}

#[test]
fn forget_resets_the_state_and_the_memory() {
    let mut rig = Rig::new();
    rig.blob(&A, "t=1/_0.seg");
    rig.manifest(&A, 2, &["_1.cfs"]);
    rig.pass(&[(A, &["_1.cfs"])]);
    assert_eq!(rig.sweeps.memory().watching(&path(&A)), 1);
    rig.sweeps.forget(A);
    assert_eq!(rig.sweeps.memory().watching(&path(&A)), 0);
    // not watched any more, so only a publish sweeps it
    assert!(rig.pass(&[]).is_empty());
    // as a first publish, and it deletes nothing on it
    let first = rig.pass(&[(A, &["_1.cfs"])]);
    assert_eq!(first.len(), 1);
    assert!(first[0].result.as_ref().unwrap().deleted.is_empty());
    assert!(rig.exists(&A, "t=1/_0.seg"));
}

#[test]
fn an_empty_pass_sends_no_request() {
    let mut rig = Rig::new();
    assert!(rig.pass(&[]).is_empty());
    assert_eq!(rig.requests(), 0);
}

#[test]
fn a_failing_shard_is_reported_and_the_others_swept_and_it_is_tried_again() {
    let mut rig = Rig::new();
    rig.manifest(&B, 2, &["_0.cfs"]);
    // A's manifest is not a manifest
    let key = format!("{}manifest", path(&A));
    rig.store.create_register(&key, b"garbage").unwrap();
    let reported = rig.pass(&[(A, &["_0.cfs"]), (B, &["_0.cfs"])]);
    assert_eq!(reported.len(), 2);
    assert!(reported[0].result.is_err());
    assert_eq!(reported[0].shard.as_ref(), A);
    assert_eq!(reported[1].result.as_ref().unwrap().outcome, Outcome::Swept);

    // repaired: A's next publish, still a first one, is swept
    rig.manifest(&A, 2, &["_0.cfs"]);
    let again = rig.pass(&[(A, &["_0.cfs"])]);
    assert_eq!(again.len(), 1);
    assert!(again[0].result.is_ok());
}

#[test]
fn a_failed_sweep_of_a_dropping_publish_is_tried_again() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs", "_1.cfs"]);
    rig.pass(&[(A, &["_0.cfs", "_1.cfs"])]);
    rig.store.fail_next(S3Op::List, 1);
    let failed = rig.pass(&[(A, &["_2.cfs"])]);
    assert!(failed[0].result.is_err());
    // the same files again: the failed sweep left the state as it was
    let retried = rig.pass(&[(A, &["_2.cfs"])]);
    assert_eq!(retried.len(), 1);
    assert!(retried[0].result.is_ok());
}

#[test]
fn a_shard_that_cannot_name_a_path_is_reported() {
    let mut rig = Rig::new();
    let bad = ShardRef {
        index: "",
        uuid: "u1",
        shard: 0,
    };
    let reported = rig.pass(&[(bad, &["_0.cfs"])]);
    assert_eq!(reported.len(), 1);
    assert!(reported[0].result.is_err());
    assert_eq!(rig.requests(), 0);
}

#[test]
fn a_pass_costs_the_sweeps_it_runs_and_nothing_for_the_shards_it_skips() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs"]);
    rig.manifest(&B, 2, &["_0.cfs"]);
    rig.pass(&[(A, &["_0.cfs"]), (B, &["_0.cfs"])]);
    let before = rig.requests();
    let lists = rig.lists();
    // A only adds, B merges: B's sweep is one register read and one LIST
    let swept = rig.pass(&[(A, &["_0.cfs", "_1.cfs"]), (B, &["_5.cfs"])]);
    assert_eq!(swept.len(), 1);
    assert_eq!(swept[0].shard.as_ref(), B);
    assert_eq!(rig.lists(), lists + 1);
    assert_eq!(rig.requests(), before + 2);
}

#[test]
fn a_skipped_add_then_a_merge_that_drops_it_is_swept() {
    let mut rig = Rig::new();
    rig.blob(&A, "t=1/_x.seg");
    rig.manifest(&A, 2, &["_0.cfs"]);
    rig.pass(&[(A, &["_0.cfs"])]);
    // X is added (skipped), then a merge drops it
    assert!(rig.pass(&[(A, &["_0.cfs", "_x.cfs"])]).is_empty());
    let merged = rig.pass(&[(A, &["_0.cfs", "_y.cfs"])]);
    assert_eq!(merged.len(), 1);
}

#[test]
fn a_file_dropped_in_the_live_term_is_swept_when_the_term_moves_on() {
    let mut rig = Rig::new();
    rig.manifest(&A, 1, &["_0.cfs", "_x.cfs"]);
    rig.pass_at(&[(A, 1, &["_0.cfs", "_x.cfs"])]);
    rig.blob(&A, "t=1/_x.seg");
    rig.manifest(&A, 1, &["_0.cfs"]);
    rig.pass_at(&[(A, 1, &["_0.cfs"])]);
    assert!(rig.exists(&A, "t=1/_x.seg"));
    // a takeover: the next publish is add-only but in term 2
    rig.manifest(&A, 2, &["_0.cfs", "_1.cfs"]);
    let moved = rig.pass_at(&[(A, 2, &["_0.cfs", "_1.cfs"])]);
    assert_eq!(moved.len(), 1);
    rig.pass(&[]);
    assert!(!rig.exists(&A, "t=1/_x.seg"));
}

#[test]
fn a_failed_sweep_is_tried_again_though_nothing_is_published() {
    let mut rig = Rig::new();
    rig.manifest(&A, 2, &["_0.cfs"]);
    rig.store.fail_next(S3Op::List, 1);
    assert!(rig.pass(&[(A, &["_0.cfs"])])[0].result.is_err());
    let retried = rig.pass(&[]);
    assert_eq!(retried.len(), 1);
    assert!(retried[0].result.is_ok());
    assert!(rig.pass(&[]).is_empty());
}
