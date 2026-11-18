//! docs/specs/node-overlay-builder: the background overlay builder over the
//! writes a writer pushed.

// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::sync::Arc;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::overlay_builder::OverlayBuilder;
use oss_node::tail_held::{KEEP_AFTER_CLOSE_MILLIS, TailHeld};
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{CommitAccess, OverlaySource};
use oss_transport::actions::tail::Record;
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::Mapping;

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

struct Commit {
    reader: DirectoryReader,
    _dir: ByteBuffersDirectory,
}

impl CommitAccess for Commit {
    fn reader(&self) -> &DirectoryReader {
        &self.reader
    }

    fn mapping_json(&self) -> &str {
        MAPPING
    }
}

/// A commit a writer made of `ids`, with the checkpoint it stores.
fn commit(ids: &[&str]) -> Arc<dyn CommitAccess> {
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
        let mut shard = Shard::new(&dir, w, mapping, 3, Box::new(|| 0)).expect("shard");
        for id in ids {
            shard
                .index(id, br#"{"n":1}"#, false, &Expect::default())
                .expect("indexed");
        }
        shard.refresh().expect("refresh");
    }
    let reader = DirectoryReader::open(&dir).expect("opens");
    Arc::new(Commit { reader, _dir: dir })
}

fn stage(sequence: i64) -> Stage {
    Stage::new(3, Some("w1".to_owned()), sequence)
}

fn index(id: &str, n: i64, seq: i64) -> Record {
    Record {
        id: id.to_owned(),
        deletion: false,
        source: format!(r#"{{"n":{n}}}"#),
        seq_no: seq,
        primary_term: 1,
        version: seq + 1,
        routing: None,
    }
}

fn budget(open_indexes: usize) -> MemoryBudget {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::OpenIndexes, open_indexes)
        .expect("fits");
    MemoryBudget::new(&config)
}

struct Fixture {
    held: Arc<TailHeld>,
    budget: MemoryBudget,
    builder: Arc<OverlayBuilder>,
    commit: Arc<dyn CommitAccess>,
}

fn fixture(open_indexes: usize) -> Fixture {
    let held = Arc::new(TailHeld::new());
    let budget = budget(open_indexes);
    let builder = OverlayBuilder::new(Arc::clone(&held), budget.clone());
    Fixture {
        held,
        budget,
        builder,
        commit: commit(&["a", "b", "c"]),
    }
}

impl Fixture {
    fn ready(&self) -> Option<Arc<oss_reader::overlay::Ready>> {
        self.builder.ready("u1", 0, &stage(1), &self.commit, 1_000)
    }

    /// The first ask tracks the shard, so records pushed after it are held.
    fn push(&self, records: &[Record]) {
        assert!(self.ready().is_none(), "nothing is finished yet");
        self.builder.settle();
        let taken = self.held.take("u1", 0, records, 1_000);
        assert!(taken.is_some(), "the shard is tracked");
    }
}

#[test]
fn ready_answers_nothing_at_once_and_the_overlay_once_built() {
    let fx = fixture(16 << 20);
    fx.push(&[]);
    assert_eq!(fx.builder.builds(), 0, "no records, nothing to build");
    fx.held
        .take("u1", 0, &[index("a", 7, 100), index("d", 8, 101)], 1_000);
    assert!(fx.ready().is_none(), "the build is not waited for");
    for _ in 0..5 {
        let _ = fx.ready();
    }
    fx.builder.settle();
    assert_eq!(fx.builder.builds(), 1, "one build however many asks");
    let ready = fx.ready().expect("finished");
    assert_eq!(ready.overlay().ids(), ["a", "d"]);
    assert_eq!(ready.overlay().doc_count(), 2);
}

#[test]
fn unchanged_records_give_the_same_overlay_and_build_nothing_more() {
    let fx = fixture(16 << 20);
    fx.push(&[index("d", 8, 100)]);
    let _ = fx.ready();
    fx.builder.settle();
    let first = fx.ready().expect("finished");
    for _ in 0..4 {
        assert!(Arc::ptr_eq(&first, &fx.ready().expect("finished")));
    }
    fx.builder.settle();
    assert_eq!(fx.builder.builds(), 1);
}

#[test]
fn new_records_build_again_and_the_old_overlay_serves_meanwhile() {
    let fx = fixture(16 << 20);
    fx.push(&[index("d", 8, 100)]);
    let _ = fx.ready();
    fx.builder.settle();
    let first = fx.ready().expect("finished");
    fx.builder.pause();
    fx.held.take("u1", 0, &[index("e", 9, 101)], 1_000);
    let meanwhile = fx.ready().expect("the old overlay");
    assert!(Arc::ptr_eq(&first, &meanwhile), "served while paused");
    fx.builder.resume();
    fx.builder.settle();
    let second = fx.ready().expect("finished");
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(second.overlay().ids(), ["d", "e"]);
    assert_eq!(fx.builder.builds(), 2);
}

#[test]
fn a_pool_that_refuses_builds_nothing_and_a_finished_overlay_holds_its_estimate() {
    let small = fixture(1_000);
    small.push(&[index("d", 8, 100)]);
    let _ = small.ready();
    small.builder.settle();
    assert!(small.ready().is_none(), "the commit serves");
    assert_eq!(small.builder.builds(), 0);
    assert_eq!(small.budget.in_use(Pool::OpenIndexes), 0);

    let roomy = fixture(16 << 20);
    roomy.push(&[index("d", 8, 100)]);
    let _ = roomy.ready();
    roomy.builder.settle();
    assert!(roomy.ready().is_some());
    assert!(roomy.budget.in_use(Pool::OpenIndexes) > 0, "charged");
    roomy.builder.closed("u1", 0, &stage(1), 2_000);
    assert_eq!(roomy.budget.in_use(Pool::OpenIndexes), 0, "returned");
}

#[test]
fn a_build_that_fails_is_not_retried_until_the_records_change() {
    let fx = fixture(16 << 20);
    fx.push(&[Record {
        source: r#"{"n":"not a number"}"#.to_owned(),
        ..index("d", 0, 100)
    }]);
    let _ = fx.ready();
    fx.builder.settle();
    assert!(fx.ready().is_none());
    for _ in 0..4 {
        let _ = fx.ready();
    }
    fx.builder.settle();
    assert_eq!(fx.builder.builds(), 1, "tried once");
    assert_eq!(fx.budget.in_use(Pool::OpenIndexes), 0, "no charge kept");
    fx.held.take("u1", 0, &[index("e", 1, 101)], 1_000);
    let _ = fx.ready();
    fx.builder.settle();
    assert_eq!(fx.builder.builds(), 2, "tried again for the new records");
}

#[test]
fn the_first_ask_tracks_the_shard_at_the_commits_checkpoint_and_closing_untracks_it() {
    let held = Arc::new(TailHeld::new());
    let builder = OverlayBuilder::new(Arc::clone(&held), budget(16 << 20));
    let commit = commit(&["a", "b", "c"]);
    let checkpoint = oss_reader::overlay::local_checkpoint(commit.reader());
    assert!(checkpoint >= 0, "the writer stores a checkpoint");
    assert_eq!(
        held.take("u1", 0, &[index("x", 1, checkpoint + 5)], 1),
        None
    );
    let _ = builder.ready("u1", 0, &stage(1), &commit, 10);
    assert_eq!(
        held.take(
            "u1",
            0,
            &[index("old", 1, checkpoint), index("new", 1, checkpoint + 1)],
            10
        ),
        Some(1),
        "at or below the checkpoint is never held"
    );
    // A close for an older stage changes nothing.
    builder.closed("u1", 0, &stage(0), 20);
    assert_eq!(
        held.take("u1", 0, &[index("n2", 1, checkpoint + 2)], 20),
        Some(1)
    );
    // A new stage tracks again in place of the old: still one engine.
    let _ = builder.ready("u1", 0, &stage(2), &commit, 30);
    assert_eq!(
        held.take("u1", 0, &[index("n3", 1, checkpoint + 3)], 30),
        Some(1)
    );
    // Closing the stage it holds lets the shard go once the keep passes.
    builder.closed("u1", 0, &stage(2), 40);
    let late = 40 + KEEP_AFTER_CLOSE_MILLIS + 1;
    assert_eq!(
        held.take("u1", 0, &[index("n4", 1, checkpoint + 4)], late),
        None
    );
}

#[test]
fn a_full_queue_refuses_a_further_build_and_the_asks_finish_later() {
    let held = Arc::new(TailHeld::new());
    let builder = OverlayBuilder::with_queue(Arc::clone(&held), budget(16 << 20), 1);
    let commit = commit(&["a"]);
    builder.pause();
    let uuids = ["u1", "u2", "u3"];
    for u in uuids {
        let _ = builder.ready(u, 0, &stage(1), &commit, 1);
        held.take(u, 0, &[index("d", 1, 100)], 1);
        assert!(builder.ready(u, 0, &stage(1), &commit, 1).is_none());
    }
    assert!(
        (1..=2).contains(&builder.pending()),
        "{} queued",
        builder.pending()
    );
    assert_eq!(builder.builds(), 0, "nothing built while paused");
    builder.resume();
    for _ in 0..10 {
        builder.settle();
        for u in uuids {
            let _ = builder.ready(u, 0, &stage(1), &commit, 1);
        }
    }
    builder.settle();
    for u in uuids {
        assert!(builder.ready(u, 0, &stage(1), &commit, 1).is_some(), "{u}");
    }
}

#[test]
fn a_build_queued_for_an_old_stage_is_dropped_when_the_stage_changed() {
    let one = fixture(16 << 20);
    one.push(&[index("d", 8, 100)]);
    let _ = one.ready();
    one.builder.settle();
    let single = one.budget.in_use(Pool::OpenIndexes);
    assert!(single > 0);

    let fx = fixture(16 << 20);
    fx.push(&[index("d", 8, 100)]);
    fx.builder.pause();
    let _ = fx.builder.ready("u1", 0, &stage(1), &fx.commit, 1_000);
    // The engine moved to another commit before the build ran.
    assert!(
        fx.builder
            .ready("u1", 0, &stage(2), &fx.commit, 1_000)
            .is_none()
    );
    fx.builder.resume();
    fx.builder.settle();
    let ready = fx.builder.ready("u1", 0, &stage(2), &fx.commit, 1_000);
    assert!(ready.is_some(), "the new stage's own build finished");
    assert_eq!(
        fx.budget.in_use(Pool::OpenIndexes),
        single,
        "the old stage's overlay was dropped with its charge"
    );
}

#[test]
fn a_refused_build_is_tried_again_once_the_pool_has_room() {
    let held = Arc::new(TailHeld::new());
    let budget = budget(1 << 20);
    let builder = OverlayBuilder::new(Arc::clone(&held), budget.clone());
    let commit = commit(&["a"]);
    // Everything but a little is taken by something else.
    let taken = budget
        .try_reserve(Pool::OpenIndexes, (1 << 20) - 1_000)
        .expect("fits");
    let _ = builder.ready("u1", 0, &stage(1), &commit, 1);
    held.take("u1", 0, &[index("d", 1, 100)], 1);
    let _ = builder.ready("u1", 0, &stage(1), &commit, 1);
    builder.settle();
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_none());
    assert_eq!(builder.builds(), 0, "refused: nothing built");
    drop(taken);
    let _ = builder.ready("u1", 0, &stage(1), &commit, 1);
    builder.settle();
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_some());
    assert_eq!(builder.builds(), 1);
}
