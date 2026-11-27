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
