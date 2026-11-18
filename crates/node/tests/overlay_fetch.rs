//! docs/specs/node-overlay-fetch: the builder asks the writer for what it
//! acknowledged when a shard first opens at a stage.

// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod overlay_rig;

use std::sync::{Arc, Mutex};

use oss_node::overlay_builder::{OverlayBuilder, TailFetcher};
use oss_node::tail_held::TailHeld;
use oss_reader::commit_push::Stage;
use oss_reader::overlay::{CommitAccess, OverlaySource, local_checkpoint};
use oss_transport::actions::tail::{FetchRequest, FetchResponse, NEW};
use overlay_rig::{budget, commit, index, stage};

/// Answers every fetch with `answer`, remembering what was asked.
struct Fetcher {
    asked: Mutex<Vec<(String, FetchRequest)>>,
    answer: Mutex<Option<FetchResponse>>,
}

impl Fetcher {
    fn new(answer: Option<FetchResponse>) -> Arc<Self> {
        Arc::new(Self {
            asked: Mutex::new(Vec::new()),
            answer: Mutex::new(answer),
        })
    }

    fn count(&self) -> usize {
        self.asked.lock().expect("lock").len()
    }
}

impl TailFetcher for Fetcher {
    fn fetch(&self, writer: &str, request: &FetchRequest) -> Option<FetchResponse> {
        self.asked
            .lock()
            .expect("lock")
            .push((writer.to_owned(), request.clone()));
        self.answer.lock().expect("lock").clone()
    }
}

fn answer(records: Vec<oss_transport::actions::tail::Record>, complete: bool) -> FetchResponse {
    FetchResponse {
        records,
        subscription: 7,
        complete,
        claimed_through: i64::MAX,
    }
}

struct Fixture {
    held: Arc<TailHeld>,
    builder: Arc<OverlayBuilder>,
    fetcher: Arc<Fetcher>,
    commit: Arc<dyn CommitAccess>,
    checkpoint: i64,
}

fn fixture(make: impl FnOnce(i64) -> Option<FetchResponse>) -> Fixture {
    let commit = commit(&["a", "b", "c"]);
    let checkpoint = local_checkpoint(commit.reader());
    let fetcher = Fetcher::new(make(checkpoint));
    let held = Arc::new(TailHeld::new());
    let builder = OverlayBuilder::fetching(
        Arc::clone(&held),
        budget(16 << 20),
        fetcher.clone(),
        "me".to_owned(),
        Arc::new(|| 1_000),
    );
    Fixture {
        held,
        builder,
        fetcher,
        commit,
        checkpoint,
    }
}

impl Fixture {
    fn ready(&self, stage: &Stage) -> Option<Arc<oss_reader::overlay::Ready>> {
        self.builder.ready("u1", 0, stage, &self.commit, 1_000)
    }
}

#[test]
fn the_first_ask_fetches_the_writer_and_builds_over_its_answer() {
    let fx = fixture(|c| Some(answer(vec![index("d", 5, c + 1)], true)));
    assert!(fx.ready(&stage(1)).is_none(), "nothing finished at once");
    assert!(fx.builder.settle(), "never settled");
    let asked = fx.fetcher.asked.lock().expect("lock").clone();
    assert_eq!(asked.len(), 1);
    let (writer, request) = &asked[0];
    assert_eq!(writer, "w1");
    assert_eq!(request.shard.index_name, "logs");
    assert_eq!(request.shard.index_uuid, "u1");
    assert_eq!(request.shard.shard, 0);
    assert_eq!(request.after_seq_no, fx.checkpoint);
    assert_eq!(request.subscriber, "me");
    assert_eq!(request.subscription, NEW);
    assert_eq!(fx.held.next_ordinal("u1", 0), 1, "the record is held");
    assert!(fx.ready(&stage(1)).is_some(), "the overlay is built");
}

#[test]
fn a_complete_answer_grants_and_an_incomplete_one_does_not() {
    let fx = fixture(|c| Some(answer(vec![index("d", 5, c + 1)], true)));
    let _ = fx.ready(&stage(1));
    assert!(fx.builder.settle());
    assert!(fx.held.granted("u1", 0, "w1", 1_000));
    let fx = fixture(|c| Some(answer(vec![index("d", 5, c + 1)], false)));
    let _ = fx.ready(&stage(1));
    assert!(fx.builder.settle());
    assert!(!fx.held.granted("u1", 0, "w1", 1_000));
}

#[test]
fn further_asks_fetch_nothing_and_a_new_stage_fetches_again() {
    let fx = fixture(|c| Some(answer(vec![index("d", 5, c + 1)], true)));
    for _ in 0..4 {
        let _ = fx.ready(&stage(1));
        assert!(fx.builder.settle());
    }
    assert_eq!(fx.fetcher.count(), 1);
    let _ = fx.ready(&stage(2));
    assert!(fx.builder.settle());
    assert_eq!(fx.fetcher.count(), 2);
}

#[test]
fn a_stage_without_a_writer_asks_nobody() {
    let fx = fixture(|_| None);
    let unstaged = Stage::new(3, None, -1);
    let _ = fx.ready(&unstaged);
    assert!(fx.builder.settle());
    assert_eq!(fx.fetcher.count(), 0);
}

#[test]
fn an_unanswered_fetch_holds_nothing_is_not_repeated_and_pushes_still_build() {
    let fx = fixture(|_| None);
    for _ in 0..3 {
        let _ = fx.ready(&stage(1));
        assert!(fx.builder.settle());
    }
    assert_eq!(fx.fetcher.count(), 1, "asked once");
    assert_eq!(fx.held.next_ordinal("u1", 0), 0);
    assert!(!fx.held.granted("u1", 0, "w1", 1_000));
    assert!(
        fx.held
            .take("u1", 0, &[index("e", 1, fx.checkpoint + 1)], 1_000)
            .is_some()
    );
    let _ = fx.ready(&stage(1));
    assert!(fx.builder.settle());
    assert!(fx.ready(&stage(1)).is_some(), "the pushed write is built");
}

#[test]
fn a_fetch_queued_for_a_stage_the_shard_left_asks_nobody() {
    let fx = fixture(|c| Some(answer(vec![index("d", 5, c + 1)], true)));
    fx.builder.pause();
    let _ = fx.ready(&stage(1));
    let _ = fx.ready(&stage(2));
    fx.builder.resume();
    assert!(fx.builder.settle());
    assert_eq!(fx.fetcher.count(), 1, "only the current stage is asked");
    let asked = fx.fetcher.asked.lock().expect("lock").len();
    assert_eq!(asked, 1);
}

#[test]
fn a_fetch_queued_for_a_closed_shard_asks_nobody() {
    let fx = fixture(|_| None);
    fx.builder.pause();
    let _ = fx.ready(&stage(1));
    fx.builder.closed("u1", 0, &stage(1), 1_000);
    fx.builder.resume();
    assert!(fx.builder.settle());
    assert_eq!(fx.fetcher.count(), 0);
}
