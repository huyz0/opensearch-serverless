//! docs/specs/request-budgets/SPEC.md AC-5: a write to a shard already in
//! the node's log epoch, held to docs/request-budgets.md on the emulator.

use crate::common;
use std::sync::Arc;
use std::time::Duration;

use common::committer::{rec, rig_on, shard, window};
use oss_blob::{BlobStore, Emulator, Rounds};
use oss_testkit::budget;
use oss_writer::log::NodeLogStore;

/// Each call is held this long, so calls sent together overlap.
const DELAY: Duration = Duration::from_millis(20);

#[test]
fn the_next_write_to_a_shard_is_one_put_in_one_round() {
    let rounds = Arc::new(Rounds::new(Emulator::new(64 << 20, 1)));
    let store: Arc<dyn BlobStore> = rounds.clone();
    let log = NodeLogStore::new(store, "", "node-1").expect("valid");
    // The rig's own emulator holds nothing here: the log writes through
    // the measured store.
    let r = rig_on(Arc::new(Emulator::new(1 << 20, 1)), log, window(0));
    r.committer
        .commit(&shard(1), 1, &[rec("first", 1)])
        .expect("lands");

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    r.committer
        .commit(&shard(1), 1, &[rec("next", 2)])
        .expect("lands");
    let detail = rounds.detail();
    budget::check(
        "write.next.requests",
        rounds.meter().total() - before,
        &detail,
    );
    budget::check(
        "write.next.round_trips",
        u64::try_from(rounds.depth()).expect("fits"),
        &detail,
    );
    rounds.reset(Duration::ZERO);
}
