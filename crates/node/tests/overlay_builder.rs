//! docs/specs/node-overlay-builder: the background overlay builder over the
//! writes a writer pushed.

// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod overlay_rig;

use std::sync::Arc;

use oss_mem::{MemoryBudget, Pool};
use oss_node::overlay_builder::OverlayBuilder;
use oss_node::tail_held::{KEEP_AFTER_CLOSE_MILLIS, TailHeld};
use oss_reader::overlay::{CommitAccess, OverlaySource};
use oss_transport::actions::tail::Record;
use overlay_rig::{budget, commit, index, stage};

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
        assert!(self.builder.settle(), "a build never finished");
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
    assert!(fx.builder.settle(), "a build never finished");
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
    assert!(fx.builder.settle(), "a build never finished");
    let first = fx.ready().expect("finished");
    for _ in 0..4 {
        assert!(Arc::ptr_eq(&first, &fx.ready().expect("finished")));
    }
    assert!(fx.builder.settle(), "a build never finished");
    assert_eq!(fx.builder.builds(), 1);
}

#[test]
fn new_records_build_again_and_the_old_overlay_serves_meanwhile() {
    let fx = fixture(16 << 20);
    fx.push(&[index("d", 8, 100)]);
    let _ = fx.ready();
    assert!(fx.builder.settle(), "a build never finished");
    let first = fx.ready().expect("finished");
    fx.builder.pause();
    fx.held.take("u1", 0, &[index("e", 9, 101)], 1_000);
    let meanwhile = fx.ready().expect("the old overlay");
    // Long enough for an unpaused worker to have built it.
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(fx.builder.builds(), 1, "held back while paused");
    assert_eq!(fx.builder.pending(), 1);
    assert!(Arc::ptr_eq(&first, &meanwhile), "served while paused");
    fx.builder.resume();
    assert!(fx.builder.settle(), "a build never finished");
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
    assert!(small.builder.settle(), "a build never finished");
    assert!(small.ready().is_none(), "the commit serves");
    assert_eq!(small.builder.builds(), 0);
    assert_eq!(small.budget.in_use(Pool::OpenIndexes), 0);

    let roomy = fixture(16 << 20);
    roomy.push(&[index("d", 8, 100)]);
    let _ = roomy.ready();
    assert!(roomy.builder.settle(), "a build never finished");
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
    assert!(fx.builder.settle(), "a build never finished");
    assert!(fx.ready().is_none());
    for _ in 0..4 {
        let _ = fx.ready();
    }
    assert!(fx.builder.settle(), "a build never finished");
    assert_eq!(fx.builder.builds(), 1, "tried once");
    assert_eq!(fx.budget.in_use(Pool::OpenIndexes), 0, "no charge kept");
    fx.held.take("u1", 0, &[index("e", 1, 101)], 1_000);
    let _ = fx.ready();
    assert!(fx.builder.settle(), "a build never finished");
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
        assert!(builder.settle(), "a build never finished");
        for u in uuids {
            let _ = builder.ready(u, 0, &stage(1), &commit, 1);
        }
    }
    assert!(builder.settle(), "a build never finished");
    for u in uuids {
        assert!(builder.ready(u, 0, &stage(1), &commit, 1).is_some(), "{u}");
    }
    assert_eq!(builder.pending(), 0, "nothing left queued");
}

#[test]
fn a_shard_with_no_records_queues_no_build() {
    let held = Arc::new(TailHeld::new());
    let builder = OverlayBuilder::new(Arc::clone(&held), budget(16 << 20));
    let commit = commit(&["a"]);
    builder.pause();
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_none());
    assert_eq!(builder.pending(), 0, "no record, no job");
    held.take("u1", 0, &[index("d", 1, 100)], 1);
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_none());
    assert_eq!(builder.pending(), 1, "a record queues one job");
    builder.resume();
    assert!(builder.settle());
    assert_eq!(builder.pending(), 0);
}

#[test]
fn a_build_queued_for_an_old_stage_is_dropped_when_the_stage_changed() {
    let one = fixture(16 << 20);
    one.push(&[index("d", 8, 100)]);
    let _ = one.ready();
    assert!(one.builder.settle(), "a build never finished");
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
    assert!(fx.builder.settle(), "a build never finished");
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
    assert!(builder.settle(), "a build never finished");
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_none());
    assert_eq!(builder.builds(), 0, "refused: nothing built");
    builder.pause();
    let _ = builder.ready("u1", 0, &stage(1), &commit, 1);
    assert_eq!(builder.pending(), 0, "no room yet: not asked again");
    builder.resume();
    drop(taken);
    let _ = builder.ready("u1", 0, &stage(1), &commit, 1);
    assert!(builder.settle(), "a build never finished");
    assert!(builder.ready("u1", 0, &stage(1), &commit, 1).is_some());
    assert_eq!(builder.builds(), 1);
}

#[test]
fn an_overlay_is_reserved_its_estimate_and_the_masks_of_the_commit_to_build() {
    // One segment of three documents: one 64-bit word of mask.
    let probe = commit(&["a", "b", "c"]);
    assert_eq!(oss_reader::overlay::mask_bytes(probe.reader()), 8);
    let record = oss_format::WalRecord::index("d", r#"{"n":8}"#, 100, 1, 101);
    let estimate = oss_reader::overlay::estimate_bytes(&[record]);

    let roomy = fixture(16 << 20);
    roomy.push(&[index("d", 8, 100)]);
    let _ = roomy.ready();
    assert!(roomy.builder.settle());
    let built = roomy.ready().expect("built");
    let kept = oss_reader::overlay::kept_bytes(
        &[oss_format::WalRecord::index("d", r#"{"n":8}"#, 100, 1, 101)],
        built.overlay().stored_bytes(),
    );
    assert!(
        kept < estimate,
        "the finished overlay keeps less than its peak"
    );
    assert_eq!(
        roomy.budget.in_use(Pool::OpenIndexes),
        kept + 8,
        "once built, the pool holds what it keeps and the masks"
    );

    // Room for the estimate alone is not room for the overlay.
    let tight = fixture(estimate + 7);
    tight.push(&[index("d", 8, 100)]);
    let _ = tight.ready();
    assert!(tight.builder.settle());
    assert_eq!(tight.builder.builds(), 0, "refused");
    let exact = fixture(estimate + 8);
    exact.push(&[index("d", 8, 100)]);
    let _ = exact.ready();
    assert!(exact.builder.settle());
    assert_eq!(exact.builder.builds(), 1, "fits exactly");
}

// docs/specs/tail-overlay-kept-charge AC-2
#[test]
fn a_finished_overlay_frees_its_peak_so_the_next_shard_builds_in_the_same_pool() {
    let record = oss_format::WalRecord::index("d", r#"{"n":8}"#, 100, 1, 101);
    let estimate = oss_reader::overlay::estimate_bytes(std::slice::from_ref(&record));
    let held = Arc::new(TailHeld::new());
    // Room for one build's peak and one overlay's masks and a little over:
    // the second shard builds only if the first gave its peak back.
    let kept_most = estimate / 2;
    let pool = estimate + 8 + kept_most;
    let builder = OverlayBuilder::new(Arc::clone(&held), budget(pool));
    let commit = commit(&["a", "b", "c"]);
    for shard in 0..2_u32 {
        assert!(
            builder
                .ready("u1", shard, &stage(1), &commit, 1_000)
                .is_none()
        );
        assert!(builder.settle());
        held.take("u1", shard, &[index("d", 8, 100)], 1_000);
        let _ = builder.ready("u1", shard, &stage(1), &commit, 1_000);
        assert!(builder.settle(), "a build never finished");
    }
    assert_eq!(builder.builds(), 2, "both shards built");
    for shard in 0..2_u32 {
        assert!(
            builder
                .ready("u1", shard, &stage(1), &commit, 1_000)
                .is_some()
        );
    }
}

/// docs/specs/node-overlay-apply AC-1 and AC-2.
#[test]
fn applying_pending_builds_what_is_held_before_it_returns() {
    let fx = fixture(16 << 20);
    fx.push(&[index("d", 8, 100)]);
    // Nothing was built yet; apply builds in the caller.
    fx.builder.apply_pending("u1", 0, &stage(1), &fx.commit);
    assert_eq!(fx.builder.builds(), 1);
    let first = fx.ready().expect("covers what is held");
    assert_eq!(first.overlay().ids(), ["d"]);
    // Nothing new is held: nothing more to build.
    fx.builder.apply_pending("u1", 0, &stage(1), &fx.commit);
    assert_eq!(fx.builder.builds(), 1);
    // A record pushed since is covered when apply returns.
    fx.held.take("u1", 0, &[index("e", 9, 101)], 1_000);
    fx.builder.apply_pending("u1", 0, &stage(1), &fx.commit);
    let second = fx.ready().expect("covers both");
    assert_eq!(second.overlay().ids(), ["d", "e"]);
    // A shard nothing tracks, and a stage it was not asked for, are left alone.
    fx.builder.apply_pending("other", 0, &stage(1), &fx.commit);
    fx.builder.apply_pending("u1", 0, &stage(2), &fx.commit);
    assert_eq!(fx.builder.builds(), 2);
}

#[test]
fn a_record_that_cannot_be_applied_marks_the_shard_skipped_so_it_holds_no_grant() {
    use oss_transport::actions::tail::{FetchResponse, NEW};
    let fx = fixture(1_000);
    fx.push(&[index("d", 8, 100)]);
    let answer = FetchResponse {
        records: Vec::new(),
        subscription: 3,
        complete: true,
        claimed_through: 100,
    };
    fx.held.answered("u1", 0, "w", NEW, 1_000, &answer);
    assert!(fx.held.granted("u1", 0, "w", 1_000));
    // The pool has no room for the overlay: the record is left out.
    fx.builder.apply_pending("u1", 0, &stage(1), &fx.commit);
    assert_eq!(fx.builder.builds(), 0);
    assert!(!fx.held.granted("u1", 0, "w", 1_000), "no grant without it");
}

/// A commit whose mapping pins an analyzer on `t` to a module that fails.
struct Pinned {
    inner: Arc<dyn CommitAccess>,
}

impl CommitAccess for Pinned {
    fn reader(&self) -> &lucene_search::directory_reader::DirectoryReader {
        self.inner.reader()
    }

    fn mapping_json(&self) -> &str {
        r#"{"properties":{"n":{"type":"long"},"t":{"type":"text","analyzer":"ext_a"}}}"#
    }

    fn index_name(&self) -> &str {
        "logs"
    }

    fn modules(&self) -> Vec<(String, Arc<dyn oss_ext::analyzer::Analyze>)> {
        struct Fails;
        impl oss_ext::analyzer::Analyze for Fails {
            fn analyze(
                &self,
                _: &str,
            ) -> Result<Vec<oss_ext::analyzer::Token>, oss_ext::CallError> {
                Err(oss_ext::CallError::busy())
            }
        }
        vec![("ext_a".to_owned(), Arc::new(Fails))]
    }
}

// docs/specs/extension-analyzer AC-8
#[test]
fn a_build_whose_module_call_fails_keeps_why_for_the_search_that_finds_the_shard_behind() {
    let mut fx = fixture(16 << 20);
    fx.commit = Arc::new(Pinned {
        inner: Arc::clone(&fx.commit),
    });
    fx.push(&[Record {
        source: r#"{"t":"some words"}"#.to_owned(),
        ..index("d", 0, 100)
    }]);
    let _ = fx.ready();
    assert!(fx.builder.settle(), "a build never finished");
    assert!(fx.ready().is_none(), "nothing is served without the tail");
    let refusal = fx
        .builder
        .extension_refusal("u1", 0, &stage(1))
        .expect("the module failure is kept");
    assert_eq!(refusal.ty, "extension_busy");
    assert!(
        fx.builder.extension_refusal("u1", 0, &stage(2)).is_none(),
        "another stage is another engine"
    );
}

#[test]
fn a_build_that_fails_for_another_reason_keeps_no_module_failure() {
    let fx = fixture(16 << 20);
    fx.push(&[Record {
        source: r#"{"n":"not a number"}"#.to_owned(),
        ..index("d", 0, 100)
    }]);
    let _ = fx.ready();
    assert!(fx.builder.settle(), "a build never finished");
    assert!(fx.builder.extension_refusal("u1", 0, &stage(1)).is_none());
}
