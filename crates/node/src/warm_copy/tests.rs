use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::Instant;

use lucene_store::page_cache::PageCacheConfig;
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_reader::ShardFiles;
use oss_testkit::index::{Spec, blobs};
use oss_transport::actions::warm::{HotFile, HotPage, PageBytes};

const PAGE: usize = 128;

fn cache(dir: &std::path::Path) -> Arc<PageCache> {
    Arc::new(
        PageCache::new(PageCacheConfig {
            disk_dir: Some(dir.to_path_buf()),
            disk_budget: 64 << 20,
            page_size: PAGE,
            ..PageCacheConfig::default()
        })
        .expect("cache"),
    )
}

/// A peer that has searched the shard, so its cache holds the pages.
struct Rig {
    cache: Arc<PageCache>,
    namespace: String,
    expected: Arc<Expected>,
}

fn rig(dir: &std::path::Path) -> Rig {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new("idx", 3000)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let store = Arc::new(store);
    let cache = cache(dir);
    let (files, meta) = ShardFiles::open_with_meta(&store, "", "idx", 0).expect("locate");
    let namespace = format!("/idx/0/t={}", files.term);
    let expected = Arc::new(Expected::new(&namespace, &files, PAGE, &|n| {
        cache.is_pinned_name(n)
    }));
    drop(
        oss_reader::open_located(
            Arc::clone(&store),
            Arc::clone(&cache),
            "",
            "idx",
            0,
            files,
            meta,
        )
        .expect("opens"),
    );
    Rig {
        cache,
        namespace,
        expected,
    }
}

fn shard() -> ShardRef {
    ShardRef {
        index_name: "idx".into(),
        index_uuid: "u".into(),
        shard: 0,
    }
}

struct Ours(Arc<Expected>);

impl Shards for Ours {
    fn expected(&self, shard: &ShardRef) -> Option<Arc<Expected>> {
        (shard.index_name == "idx").then(|| Arc::clone(&self.0))
    }
}

/// Serves from a warmed cache; `corrupt` flips the pages asked at that call.
struct Fake<'a> {
    rig: &'a Rig,
    page_size: i32,
    answers: Cell<bool>,
    corrupt_from_call: Option<usize>,
    calls: Cell<usize>,
    /// The first this many page calls fail, as a busy peer's refusals do.
    refuse_first: Cell<usize>,
    /// Every page call fails.
    refuse_all: Cell<bool>,
}

impl<'a> Fake<'a> {
    fn new(rig: &'a Rig) -> Self {
        Self {
            rig,
            page_size: PAGE as i32,
            answers: Cell::new(true),
            corrupt_from_call: None,
            calls: Cell::new(0),
            refuse_first: Cell::new(0),
            refuse_all: Cell::new(false),
        }
    }
}

impl Peer for Fake<'_> {
    fn hot_set(&self, _: &HotSetRequest) -> Option<HotSetResponse> {
        if !self.answers.get() {
            return None;
        }
        let prefix = format!("{}\u{0}", self.rig.namespace);
        let resident = self
            .rig
            .cache
            .resident_pages(&[prefix.as_str()], 10_000)
            .remove(0);
        let mut by_key: BTreeMap<String, Vec<HotPage>> = BTreeMap::new();
        for page in resident {
            let name = page.key.split('\u{0}').nth(1).expect("name").to_owned();
            if self.rig.cache.is_pinned_name(&name) || !page.key.contains("\u{0}p\u{0}") {
                continue;
            }
            by_key
                .entry(page.key.to_string())
                .or_default()
                .push(HotPage {
                    page: page.page,
                    len: page.len,
                });
        }
        let files = by_key
            .into_iter()
            .map(|(key, pages)| HotFile { key, pages })
            .collect();
        Some(HotSetResponse {
            page_size: self.page_size,
            shards: vec![(shard(), files)],
        })
    }

    fn pages(&self, request: &PagesRequest) -> Option<PagesResponse> {
        let call = self.calls.get();
        self.calls.set(call.saturating_add(1));
        if !self.answers.get() || self.refuse_all.get() {
            return None;
        }
        if self.refuse_first.get() > 0 {
            self.refuse_first
                .set(self.refuse_first.get().saturating_sub(1));
            return None;
        }
        let pages = request
            .asks
            .iter()
            .filter_map(|ask| {
                let mut bytes = self.rig.cache.peek_page(&ask.key, ask.page)?.to_vec();
                if self.corrupt_from_call.is_some_and(|c| call >= c) {
                    bytes.pop();
                }
                Some(PageBytes {
                    key: ask.key.clone(),
                    page: ask.page,
                    bytes,
                })
            })
            .collect();
        Some(PagesResponse { pages })
    }
}

#[derive(Default)]
struct Sleeps(RefCell<Vec<Duration>>);

impl Pause for Sleeps {
    fn pause(&self, duration: Duration) {
        self.0.borrow_mut().push(duration);
    }
}

fn settings(rate: u64) -> Settings<'static> {
    Settings {
        joiner: "joiner",
        max_shards: 1000,
        max_bytes: 1 << 30,
        bytes_per_second: rate,
        open_parallel: 16,
    }
}

fn never() -> bool {
    false
}

// docs/specs/reader-warm-copy AC-4
#[test]
fn every_named_page_is_taken_and_the_pauses_add_up_to_the_byte_rate() {
    let a = oss_testkit::TempDir::new("warm-copy-peer");
    let b = oss_testkit::TempDir::new("warm-copy-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let fake = Fake::new(&peer);
    let sleeps = Sleeps::default();
    let progress = Progress::default();
    let rate = 1 << 20;
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(rate),
        &sleeps,
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Done);
    assert!(progress.named() > 0);
    assert_eq!(progress.copied(), progress.named());
    assert!(progress.reached(0.9));
    let prefix = format!("{}\u{0}", peer.namespace);
    let theirs = peer
        .cache
        .resident_pages(&[prefix.as_str()], 10_000)
        .remove(0);
    for page in theirs.iter().filter(|p| {
        p.key.contains("\u{0}p\u{0}")
            && !peer
                .cache
                .is_pinned_name(p.key.split('\u{0}').nth(1).unwrap())
    }) {
        assert_eq!(
            joiner.peek_page(&page.key, page.page).map(|b| b.to_vec()),
            peer.cache
                .peek_page(&page.key, page.page)
                .map(|b| b.to_vec()),
            "{} page {}",
            page.key,
            page.page
        );
    }
    let slept: Duration = sleeps.0.borrow().iter().sum();
    let wanted = pause_for(progress.copied(), rate);
    assert!(slept >= wanted, "{slept:?} < {wanted:?}");
    assert!(fake.calls.get() > 0);
}

// docs/specs/reader-warm-copy AC-6
#[test]
fn a_peer_that_does_not_answer_or_names_another_page_size_gives_nothing() {
    let a = oss_testkit::TempDir::new("warm-copy-silent");
    let b = oss_testkit::TempDir::new("warm-copy-silent-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let mut fake = Fake::new(&peer);
    fake.answers.set(false);
    let progress = Progress::default();
    let go = |fake: &Fake<'_>| {
        copy_from_peer(
            fake,
            &Ours(Arc::clone(&peer.expected)),
            &joiner,
            &settings(1 << 20),
            &Sleeps::default(),
            &progress,
            &never,
        )
    };
    assert_eq!(go(&fake), Outcome::Unreachable);
    fake.answers.set(true);
    fake.page_size = 4096;
    assert_eq!(go(&fake), Outcome::Unreachable);
    assert_eq!((progress.named(), progress.copied()), (0, 0));
    assert_eq!(fake.calls.get(), 0, "no page was asked");
}

// docs/specs/reader-warm-copy AC-6, AC-9
#[test]
fn a_short_page_ends_that_peers_copy_and_the_target_drops_what_it_did_not_give() {
    let a = oss_testkit::TempDir::new("warm-copy-bad");
    let b = oss_testkit::TempDir::new("warm-copy-bad-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let mut fake = Fake::new(&peer);
    fake.corrupt_from_call = Some(0);
    let progress = Progress::default();
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Bad);
    assert_eq!(
        progress.named(),
        progress.copied(),
        "only what was taken is waited for"
    );
}

// docs/specs/reader-warm-copy AC-6, AC-7
#[test]
fn a_peer_failing_after_some_batches_keeps_what_it_gave_and_is_no_longer_waited_for() {
    let a = oss_testkit::TempDir::new("warm-copy-midway");
    let b = oss_testkit::TempDir::new("warm-copy-midway-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let mut fake = Fake::new(&peer);
    fake.corrupt_from_call = Some(1);
    let progress = Progress::default();
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert!(
        fake.calls.get() >= 2,
        "the failure came after a first batch"
    );
    assert_eq!(outcome, Outcome::Bad);
    assert!(progress.copied() > 0, "the first batch was kept");
    assert_eq!(
        progress.named(),
        progress.copied(),
        "the target is what the peer gave, so the node is not held for the rest"
    );
    assert!(progress.reached(0.9));
}

// docs/specs/reader-warm-copy AC-6
#[test]
fn a_copy_stops_when_the_wait_is_over() {
    let a = oss_testkit::TempDir::new("warm-copy-expired");
    let b = oss_testkit::TempDir::new("warm-copy-expired-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let fake = Fake::new(&peer);
    let progress = Progress::default();
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &|| true,
    );
    assert_eq!(outcome, Outcome::Stopped);
    assert_eq!(progress.copied(), 0);
    assert!(
        progress.named() > 0,
        "the list was taken before the wait ended"
    );
}

#[test]
fn nothing_named_is_reached_and_a_fraction_is_taken_of_what_is() {
    let progress = Progress::default();
    assert!(progress.reached(0.9));
    progress.named.store(100, Ordering::Release);
    assert!(!progress.reached(0.9));
    progress.copied.store(90, Ordering::Release);
    assert!(progress.reached(0.9));
}

mod applies;
mod two_peers;

// docs/specs/reader-warm-copy AC-6
#[test]
fn a_busy_peers_refusals_are_retried_after_a_backoff_and_the_copy_still_completes() {
    let a = oss_testkit::TempDir::new("warm-copy-busy");
    let b = oss_testkit::TempDir::new("warm-copy-busy-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let fake = Fake::new(&peer);
    // The first batch is refused twice, as a peer serving its limit does.
    fake.refuse_first.set(2);
    let progress = Progress::default();
    let sleeps = Sleeps::default();
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 40),
        &sleeps,
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Done);
    assert!(progress.copied() > 0);
    assert_eq!(
        progress.named(),
        progress.copied(),
        "everything named was taken"
    );
    let waits = sleeps.0.borrow();
    assert_eq!(
        &waits[..2],
        [Duration::from_millis(25), Duration::from_millis(50)],
        "each retry waits twice as long as the one before"
    );
}

// docs/specs/reader-warm-copy AC-6
#[test]
fn a_peer_that_keeps_refusing_is_given_up_on_after_a_bounded_number_of_tries() {
    let a = oss_testkit::TempDir::new("warm-copy-refuses");
    let b = oss_testkit::TempDir::new("warm-copy-refuses-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let fake = Fake::new(&peer);
    fake.refuse_all.set(true);
    let progress = Progress::default();
    let sleeps = Sleeps::default();
    let outcome = copy_from_peer(
        &fake,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 40),
        &sleeps,
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Unreachable);
    assert_eq!(
        fake.calls.get(),
        PAGES_ATTEMPTS,
        "the first batch only, tried a bounded number of times"
    );
    assert_eq!(
        sleeps.0.borrow().len(),
        PAGES_ATTEMPTS - 1,
        "a backoff between tries"
    );
    assert_eq!(
        progress.named(),
        0,
        "what the peer would not give is not waited for"
    );
}
