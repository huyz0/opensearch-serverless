//! The copy across several peers and many shards.

use super::*;
use std::sync::atomic::AtomicBool;

// docs/specs/reader-warm-copy AC-7
#[test]
fn a_peer_failing_after_m_bytes_leaves_the_target_at_the_other_peers_bytes_plus_m() {
    use crate::lifecycle::Readiness;
    use crate::warm_boot::{Join, finish_when_warm};

    let a = oss_testkit::TempDir::new("warm-copy-two-peers");
    let b = oss_testkit::TempDir::new("warm-copy-two-peers-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let progress = Progress::default();
    let ours = Ours(Arc::clone(&peer.expected));

    let good = Fake::new(&peer);
    let first = copy_from_peer(
        &good,
        &ours,
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(first, Outcome::Done);
    let others = progress.named();
    assert!(others > 0);

    let mut failing = Fake::new(&peer);
    failing.corrupt_from_call = Some(1);
    let second = copy_from_peer(
        &failing,
        &ours,
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(second, Outcome::Bad);
    let m = progress.copied() - others;
    assert!(m > 0, "the failing peer gave a first batch");
    assert!(
        m < others,
        "it named as many bytes as the good peer and gave fewer"
    );
    assert_eq!(
        progress.named(),
        others + m,
        "the target is the other peers' bytes plus what the failing one gave"
    );
    assert_eq!(progress.copied(), progress.named());

    // The node is let in at once: nothing it is waiting for is missing.
    let readiness = Readiness::warming();
    let clock = Sleeps::default();
    let expired = AtomicBool::new(false);
    let waited = || clock.0.borrow().iter().sum::<Duration>();
    let timed_out = finish_when_warm(
        &readiness,
        &progress,
        &AtomicBool::new(false),
        &Join {
            ready_percent: 90,
            max_wait: Duration::from_secs(60),
            poll: Duration::from_secs(1),
        },
        &waited,
        &clock,
        &expired,
    );
    assert!(!timed_out);
    assert!(readiness.ready());
    assert_eq!(
        waited(),
        Duration::ZERO,
        "no wait for the failed peer's rest"
    );
}

/// A peer naming many shards and no pages.
struct Listing(usize);

impl Peer for Listing {
    fn hot_set(&self, _: &HotSetRequest) -> Option<HotSetResponse> {
        Some(HotSetResponse {
            page_size: PAGE as i32,
            shards: (0..self.0)
                .map(|n| {
                    (
                        ShardRef {
                            index_name: "idx".into(),
                            index_uuid: "u".into(),
                            shard: i32::try_from(n).expect("small"),
                        },
                        Vec::new(),
                    )
                })
                .collect(),
        })
    }

    fn pages(&self, _: &PagesRequest) -> Option<PagesResponse> {
        None
    }
}

/// Opening a shard takes a while, as reading its commit from the store does;
/// it counts how many are open at once.
struct Opening {
    now: std::sync::atomic::AtomicUsize,
    most: std::sync::atomic::AtomicUsize,
    opened: std::sync::atomic::AtomicUsize,
    expected: Arc<Expected>,
}

impl Shards for Opening {
    fn expected(&self, _: &ShardRef) -> Option<Arc<Expected>> {
        use std::sync::atomic::Ordering::SeqCst;
        let now = self.now.fetch_add(1, SeqCst).saturating_add(1);
        self.most.fetch_max(now, SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        self.now.fetch_sub(1, SeqCst);
        self.opened.fetch_add(1, SeqCst);
        Some(Arc::clone(&self.expected))
    }
}

// docs/specs/reader-warm-copy AC-5, AC-7
#[test]
fn the_peers_shards_are_opened_side_by_side_and_each_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let a = oss_testkit::TempDir::new("warm-copy-open-peer");
    let b = oss_testkit::TempDir::new("warm-copy-open-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let shards = Opening {
        now: 0.into(),
        most: 0.into(),
        opened: 0.into(),
        expected: Arc::clone(&peer.expected),
    };
    let count = 64;
    let started = Instant::now();
    let outcome = copy_from_peer(
        &Listing(count),
        &shards,
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &Progress::default(),
        &never,
    );
    let took = started.elapsed();
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(shards.opened.load(SeqCst), count, "each shard opened once");
    let most = shards.most.load(SeqCst);
    assert!(
        (2..=OPEN_PARALLEL).contains(&most),
        "{most} open at once, to be more than one and at most {OPEN_PARALLEL}"
    );
    assert!(
        took < Duration::from_millis(40_u64.saturating_mul(64) / 3),
        "{took:?} for {count} shards opened one after another takes {:?}",
        Duration::from_millis(40_u64.saturating_mul(64))
    );
}
