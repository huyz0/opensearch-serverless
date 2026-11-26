use super::*;
use std::sync::atomic::AtomicU64;

use lucene_store::page_cache::PageCacheConfig;
use oss_mem::{BudgetConfig, SMALL_BUFFER};
use oss_transport::actions::tail::ShardRef;

use crate::warm_expect::Expected;

/// Answers no hot set, but notes how many are being asked at once.
struct Probe {
    now: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
    asked: Arc<AtomicU64>,
}

impl Peer for Probe {
    fn hot_set(&self, _: &HotSetRequest) -> Option<HotSetResponse> {
        let now = self.now.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        self.most.fetch_max(now, Ordering::AcqRel);
        self.asked.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(Duration::from_millis(30));
        self.now.fetch_sub(1, Ordering::AcqRel);
        None
    }

    fn pages(&self, _: &PagesRequest) -> Option<PagesResponse> {
        None
    }
}

struct None_;

impl Shards for None_ {
    fn expected(&self, _: &ShardRef) -> Option<Arc<Expected>> {
        None
    }
}

struct Idle;

impl Pause for Idle {
    fn pause(&self, _: Duration) {}
}

fn run(peers: usize, parallel: usize, pool_bytes: usize) -> (Vec<Outcome>, usize, u64) {
    let dir = oss_testkit::TempDir::new("warm-join");
    let cache = PageCache::new(PageCacheConfig {
        disk_dir: Some(dir.path().to_path_buf()),
        disk_budget: 1 << 20,
        ..PageCacheConfig::default()
    })
    .expect("cache");
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, pool_bytes)
            .expect("fits"),
    );
    let now = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let asked = Arc::new(AtomicU64::new(0));
    let list: Vec<Box<dyn Peer + Send + Sync>> = (0..peers)
        .map(|_| {
            Box::new(Probe {
                now: Arc::clone(&now),
                most: Arc::clone(&most),
                asked: Arc::clone(&asked),
            }) as Box<dyn Peer + Send + Sync>
        })
        .collect();
    let plan = Plan {
        settings: Settings {
            joiner: "j",
            max_shards: 1,
            max_bytes: 1,
            bytes_per_second: 1,
            open_parallel: 1,
        },
        parallel_peers: parallel,
        pool: Pool::Transport,
        pause: &Idle,
        expired: &|| false,
    };
    let outcomes = warm_from(&list, &None_, &cache, &budget, &plan, &Progress::default());
    let _ = SMALL_BUFFER;
    (
        outcomes,
        most.load(Ordering::Acquire),
        asked.load(Ordering::Acquire),
    )
}

// docs/specs/reader-warm-copy AC-4
#[test]
fn every_peer_is_asked_and_never_more_than_parallel_peers_at_once() {
    let (outcomes, most, asked) = run(6, 2, 64 << 20);
    assert_eq!(outcomes.len(), 6);
    assert!(outcomes.iter().all(|o| *o == Outcome::Unreachable));
    assert_eq!(asked, 6);
    assert_eq!(most, 2, "two at once, never more");
}

// docs/specs/reader-warm-copy AC-4
#[test]
fn a_joiner_whose_batches_do_not_fit_the_pool_asks_no_peer() {
    let (outcomes, _, asked) = run(4, 2, BATCH_BYTES * 2 - 1);
    assert!(outcomes.is_empty());
    assert_eq!(asked, 0);
    let (outcomes, _, asked) = run(4, 2, BATCH_BYTES * 2);
    assert_eq!((outcomes.len(), asked), (4, 4), "exactly the batches fit");
}

#[test]
fn the_peers_are_the_other_live_search_nodes() {
    use oss_format::NodeLease;
    struct Fixed(Vec<NodeLease>);
    impl Leases for Fixed {
        fn current(&self) -> Arc<[NodeLease]> {
            Arc::from(self.0.clone())
        }
    }
    let lease = |id: &str, role: &str, revoked: bool| {
        let mut l = NodeLease::new(id, "e", "127.0.0.1:1", 0);
        l.set_roles([role.to_owned()]);
        l.revoked = revoked;
        l
    };
    let fixed = Fixed(vec![
        lease("me", ROLE_SEARCH, false),
        lease("a", ROLE_SEARCH, false),
        lease("w", "write", false),
        lease("gone", ROLE_SEARCH, true),
        lease("b", ROLE_SEARCH, false),
    ]);
    assert_eq!(peers_of(&fixed, "me"), ["a", "b"]);
}
