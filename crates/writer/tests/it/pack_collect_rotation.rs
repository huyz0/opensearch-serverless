//! A cluster of collectors, each its own process with its own memory: a pack
//! no one names is deleted within a bounded number of windows however many
//! members there are, and a pack that is named is asked about few times, not
//! once a window.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, Emulator, SliceSource};
use oss_format::node_pack_key;
use oss_writer::pack::{
    PackCollector, PackError, PackId, PackIndexStore, PackLedger, PackReferences, WINDOW_MILLIS,
    window_of,
};

const MIN: u64 = 60_000;
const NODE: &str = "writer-1";

#[derive(Default)]
struct Counting {
    asked: AtomicUsize,
    named: bool,
}

impl PackReferences for Counting {
    fn names(&self, _: &PackId<'_>) -> Result<bool, PackError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(self.named)
    }
}

fn members(n: usize) -> Vec<String> {
    let mut all: Vec<String> = (0..n).map(|i| format!("node-{i:04}")).collect();
    all.sort();
    all
}

/// A dead writer's one old pack in a cluster of `n` collectors, each with its
/// own memory, over `windows` windows from 100. Returns the window count at
/// which the pack went (`None` if it stayed) and the references asked.
fn run_cluster(n: usize, windows: u64, named: bool) -> (Option<u64>, usize) {
    let e = Arc::new(Emulator::new(1 << 30, 7));
    let store = Arc::clone(&e) as Arc<dyn BlobStore>;
    PackIndexStore::new(Arc::clone(&store), "")
        .add(NODE, 1)
        .expect("indexed");
    PackLedger::new(Arc::clone(&store), "", NODE)
        .expect("ledger")
        .reserve(1, 8)
        .expect("reserved");
    let key = node_pack_key("", NODE, 1, 0).expect("key");
    e.put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
        .expect("put");
    e.advance_clock(16 * MIN);
    let all = members(n);
    let mut collectors: Vec<PackCollector> = all
        .iter()
        .map(|_| PackCollector::new(Arc::clone(&store), "", 100))
        .collect();
    let refs = Counting {
        named,
        ..Counting::default()
    };
    let mut gone = None;
    for w in 100..100_u64.saturating_add(windows) {
        let now = w.saturating_mul(WINDOW_MILLIS).saturating_add(MIN);
        assert_eq!(window_of(now), w);
        for (me, c) in all.iter().zip(collectors.iter_mut()) {
            c.tick(me, &all, now, &refs).expect("tick");
        }
        if gone.is_none() && e.head(&key).expect("head").is_none() {
            gone = Some(w.saturating_sub(99));
        }
    }
    (gone, refs.asked.load(Ordering::SeqCst))
}

/// With 50 members the pack goes within a few windows, not whenever one
/// member happens to be chosen twice.
#[test]
fn a_pack_no_one_names_goes_within_a_few_windows_in_a_big_cluster() {
    let (gone, _) = run_cluster(50, 12, false);
    assert!(
        gone.is_some_and(|w| w <= 12),
        "went after {gone:?} windows with 50 members"
    );
}

/// A named pack is asked about a handful of times over a day of windows, not
/// once a window, however many members there are.
#[test]
fn a_named_pack_is_asked_about_a_few_times_a_day_in_a_big_cluster() {
    let (_, asked) = run_cluster(50, 144, true);
    assert!(asked <= 12, "asked {asked} times in a day with 50 members");
}

/// The tracking cap belongs to the collector, not to each node it judges:
/// with a cap of four, two nodes with three old packs each have four packs
/// tracked between them, so four are deleted at the second pass and the rest
/// follow once the first are gone.
#[test]
fn the_tracking_cap_is_the_collectors_across_every_node() {
    let e = Arc::new(Emulator::new(1 << 30, 7));
    let store = Arc::clone(&e) as Arc<dyn BlobStore>;
    for node in ["node-a", "node-b"] {
        PackIndexStore::new(Arc::clone(&store), "")
            .add(node, 1)
            .expect("indexed");
        PackLedger::new(Arc::clone(&store), "", node)
            .expect("ledger")
            .reserve(1, 8)
            .expect("reserved");
        for ordinal in 0..3 {
            let key = node_pack_key("", node, 1, ordinal).expect("key");
            e.put(&key, &mut SliceSource::new(b"pack"), &mut [0; 16])
                .expect("put");
        }
    }
    e.advance_clock(16 * MIN);
    let me = vec!["collector".to_owned()];
    let mut c = PackCollector::new(Arc::clone(&store), "", 4);
    let refs = Counting::default();
    let mut deleted = Vec::new();
    for w in 100..108 {
        let now = w * WINDOW_MILLIS + MIN;
        let out = c.tick("collector", &me, now, &refs).expect("tick");
        deleted.push(out.deleted);
    }
    assert_eq!(deleted.iter().take(2).sum::<usize>(), 4, "{deleted:?}");
    assert_eq!(deleted.iter().sum::<usize>(), 6, "all go in the end");
}
