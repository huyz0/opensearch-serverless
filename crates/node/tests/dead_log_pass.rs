//! docs/specs/node-dead-log-pass: a node collects the logs of the dead nodes
//! it reaps, on a wait that backs off, and makes no request for the rest.

mod dead_rig;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use dead_rig::{HOUR, Later, MAPPING, info, membership, node, write};
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_cluster::heads::ShardHeadStore;
use oss_cluster::{LeaseClock, LivenessOracle, SystemClock};
use oss_format::IndexRecord;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::writer::{
    DeadLogParts, DeadLogPass, DeadLogReport, DeadMailbox, DeadSnapshot, REAPERS, Takeover,
};

const FIRST_WAIT: i64 = 1_000;

struct Rig {
    store: Arc<Emulator>,
    pass: DeadLogPass,
    snapshot: DeadSnapshot,
    now: Arc<AtomicI64>,
    one: Arc<oss_node::writer::WriterRole>,
    descriptors: Arc<DescriptorStore>,
}

fn rig(collector: &str) -> Rig {
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

    // node-2 is the live one a clock an hour on sees.
    let later = membership(&store, Arc::new(Later));
    let mut lease = oss_format::NodeLease::new("node-2", "e2", "a", 0);
    lease.set_roles(["ingest".to_owned()]);
    later.renew(&lease).expect("enrolled");
    let refreshed = later.refresh().expect("refreshes");
    assert_eq!(
        refreshed.dead_with_epochs.len(),
        1,
        "node-1 names its epoch"
    );
    let heads = Arc::new(
        ShardHeadStore::new(
            Arc::clone(&dynstore),
            "",
            Arc::new(Later),
            30_000,
            Arc::new(LivenessOracle::new(Arc::clone(&later))),
        )
        .expect("heads"),
    );
    let now = Arc::new(AtomicI64::new(
        SystemClock.now_millis().saturating_add(HOUR),
    ));
    let clock = Arc::clone(&now);
    let pass = DeadLogPass::new(
        DeadLogParts {
            membership: later,
            heads,
            descriptors: Arc::clone(&descriptors),
            store: dynstore,
            base: String::new(),
            node_id: collector.to_owned(),
            term: 2,
            budget: MemoryBudget::new(
                &BudgetConfig::new(256 << 20)
                    .with_pool(Pool::RequestBodies, 64 << 20)
                    .expect("pool"),
            ),
            buffer_bytes: 1 << 16,
            first_wait_millis: FIRST_WAIT,
            predecessors: None,
        },
        Box::new(move || clock.load(Ordering::SeqCst)),
    );
    Rig {
        store,
        pass,
        snapshot: DeadSnapshot {
            live: refreshed.live,
            dead: refreshed.dead_with_epochs,
        },
        now,
        one,
        descriptors,
    }
}

fn requests(store: &Emulator) -> u64 {
    [S3Op::Get, S3Op::Put, S3Op::List, S3Op::Delete]
        .into_iter()
        .map(|op| store.meter().count(op))
        .sum()
}

#[test]
fn a_node_that_is_not_a_reaper_makes_no_request() {
    let r = rig("node-9");
    let before = requests(&r.store);
    let report = r.pass.run(&r.snapshot, false);
    assert_eq!((report.considered, report.mine), (1, 0));
    assert_eq!(requests(&r.store), before);
    assert_eq!(REAPERS, 2);
}

#[test]
fn a_reaper_waits_out_a_log_that_is_still_needed_then_collects_it() {
    let r = rig("node-2");
    // The head is still node-1's: still needed, and a wait starts.
    let report = r.pass.run(&r.snapshot, false);
    assert_eq!((report.mine, report.still_needed), (1, 1), "{report:?}");
    let before = requests(&r.store);
    let again = r.pass.run(&r.snapshot, false);
    assert_eq!((again.mine, again.waiting, again.still_needed), (1, 1, 0));
    assert_eq!(requests(&r.store), before, "a waiting log costs nothing");

    // Past the first wait it is tried again, and the second wait is twice the first.
    r.now.fetch_add(FIRST_WAIT, Ordering::SeqCst);
    assert_eq!(r.pass.run(&r.snapshot, false).still_needed, 1);
    r.now.fetch_add(FIRST_WAIT, Ordering::SeqCst);
    assert_eq!(
        r.pass.run(&r.snapshot, false).waiting,
        1,
        "the wait doubled"
    );

    // A successor publishes everything the log held.
    let dynstore: Arc<dyn BlobStore> = r.store.clone();
    let two = node(
        &r.store,
        &r.descriptors,
        "node-2",
        2,
        true,
        Some(Takeover::from_leases(dynstore, "", "node-2", 2, 1 << 20)),
    );
    assert_eq!(write(&two, "c"), 2);
    r.now.fetch_add(10 * FIRST_WAIT, Ordering::SeqCst);

    let dry = r.pass.run(&r.snapshot, true);
    assert_eq!(dry.would_collect, 1);
    r.now.fetch_add(100 * FIRST_WAIT, Ordering::SeqCst);
    let lists = r.store.meter().count(S3Op::List);
    let real = r.pass.run(&r.snapshot, false);
    assert_eq!(
        real,
        DeadLogReport {
            considered: 1,
            mine: 1,
            collected: 1,
            ..DeadLogReport::default()
        }
    );
    assert_eq!(r.store.meter().count(S3Op::List), lists, "no LIST");
    drop(r.one);
}

#[test]
fn the_mailbox_hands_over_the_latest_snapshot_once() {
    let mailbox = DeadMailbox::default();
    assert!(mailbox.take().is_none());
    for dead in [0, 1] {
        mailbox.post(DeadSnapshot {
            live: Arc::from(Vec::new()),
            dead: (0..dead)
                .map(|_| oss_format::NodeLease::new("n", "e", "a", 0))
                .collect(),
        });
    }
    assert_eq!(
        mailbox.take().map(|s| s.dead.len()),
        Some(1),
        "the later one"
    );
    assert!(mailbox.take().is_none());
}

#[test]
fn the_wait_stops_doubling_at_sixty_four_times_the_first() {
    let r = rig("node-2");
    let cap = FIRST_WAIT * 64;
    for _ in 0..10 {
        assert_eq!(r.pass.run(&r.snapshot, false).still_needed, 1);
        r.now.fetch_add(cap, Ordering::SeqCst);
    }
    // The last try's wait is the cap: a moment short of it is waiting, the cap itself is not.
    assert_eq!(r.pass.run(&r.snapshot, false).still_needed, 1);
    r.now.fetch_add(cap - 1, Ordering::SeqCst);
    assert_eq!(r.pass.run(&r.snapshot, false).waiting, 1);
    r.now.fetch_add(1, Ordering::SeqCst);
    assert_eq!(r.pass.run(&r.snapshot, false).still_needed, 1);
}
