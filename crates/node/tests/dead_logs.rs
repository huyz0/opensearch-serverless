//! docs/specs/node-dead-logs: the janitor's collector over the store-backed
//! log and commit seams.

mod dead_rig;

use std::sync::Arc;

use dead_rig::{HOUR, Later, MAPPING, info, membership, node, write};
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LivenessOracle, SystemClock};
use oss_format::{IndexRecord, ShardRef};
use oss_janitor::node_log::{Collected, Collector};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{StoreCommits, StoreNodeLogs, Takeover};
use oss_writer::takeover::commit_checkpoint;

#[test]
fn a_dead_nodes_log_is_collected_once_a_successor_has_published_all_of_it() {
    let store = Arc::new(Emulator::new(64 << 20, 1));
    let dynstore: Arc<dyn BlobStore> = store.clone();
    let descriptors = Arc::new(
        DescriptorStore::new(Arc::clone(&dynstore), "", DeleteMode::Tombstone)
            .expect("descriptors"),
    );
    let mut record = IndexRecord::new("logs", "u1", 1);
    record.mapping = Some(std::borrow::Cow::Borrowed(MAPPING));
    descriptors
        .create(&record, 1, &mut vec![0u8; 1 << 16])
        .expect("created");

    let one = node(&store, &descriptors, "node-1", 1, false, None);
    assert_eq!(write(&one, "a"), 0);
    one.refresh(&info(), 0).expect("published");
    assert_eq!(write(&one, "b"), 1);

    let later = membership(&store, Arc::new(Later));
    let heads = ShardHeadStore::new(
        Arc::clone(&dynstore),
        "",
        Arc::new(Later),
        30_000,
        Arc::new(LivenessOracle::new(Arc::clone(&later))),
    )
    .expect("heads");
    let logs = StoreNodeLogs::new(Arc::clone(&dynstore), "", "node-2", 2);
    let commits = StoreCommits::new(Arc::clone(&dynstore), "");
    let clock = || SystemClock.now_millis().saturating_add(HOUR);
    let mut buf = vec![0u8; 1 << 16];
    let pass = |dry_run: bool, buf: &mut [u8]| {
        Collector::new(
            &later,
            &heads,
            &descriptors,
            &logs,
            &commits,
            &clock,
            dry_run,
        )
        .collect("node-1", buf)
        .expect("collects")
    };

    // b is only in the log, and the head is still node-1's.
    assert_eq!(pass(false, &mut buf), Collected::StillNeeded);

    let two = node(
        &store,
        &descriptors,
        "node-2",
        2,
        true,
        Some(Takeover::from_leases(
            Arc::clone(&dynstore),
            "",
            "node-2",
            2,
            1 << 20,
        )),
    );
    assert_eq!(write(&two, "c"), 2, "node-2 replayed a and b");

    let at = ShardRef {
        index: "logs",
        uuid: "u1",
        shard: 0,
    };
    assert_eq!(
        commit_checkpoint(store.as_ref(), "", &at).expect("reads"),
        Some(Some(1)),
        "b was published under the new term"
    );

    assert_eq!(pass(true, &mut buf), Collected::WouldCollect);
    let lists = store.meter().count(S3Op::List);
    assert_eq!(pass(false, &mut buf), Collected::Collected);
    assert_eq!(store.meter().count(S3Op::List), lists, "no LIST to collect");
    assert_eq!(
        pass(false, &mut buf),
        Collected::Nothing,
        "the lease forgot it"
    );
}
