//! The copy loop against a peer's answers: what it asks, what it accepts,
//! when it sends, and what a refusing peer leaves.

use super::*;
use std::sync::atomic::AtomicBool;

/// Wraps a served peer, recording each ask and adding to what it answers.
struct Tampered<'a> {
    inner: Fake<'a>,
    /// Added to the hot set: a file key, and a page and length it names.
    extra_named: Vec<(String, u64, u32)>,
    /// Added to every answer.
    extra_served: Vec<PageBytes>,
    asked: RefCell<Vec<(String, u64)>>,
}

impl<'a> Tampered<'a> {
    fn new(rig: &'a Rig) -> Self {
        Self {
            inner: Fake::new(rig),
            extra_named: Vec::new(),
            extra_served: Vec::new(),
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl Peer for Tampered<'_> {
    fn hot_set(&self, request: &HotSetRequest) -> Option<HotSetResponse> {
        let mut listed = self.inner.hot_set(request)?;
        for (key, page, len) in &self.extra_named {
            if let Some((_, files)) = listed.shards.first_mut() {
                files.push(HotFile {
                    key: key.clone(),
                    pages: vec![HotPage {
                        page: *page,
                        len: *len,
                    }],
                });
            }
        }
        Some(listed)
    }

    fn pages(&self, request: &PagesRequest) -> Option<PagesResponse> {
        self.asked
            .borrow_mut()
            .extend(request.asks.iter().map(|a| (a.key.clone(), a.page)));
        let mut answer = self.inner.pages(request)?;
        answer.pages.extend(self.extra_served.iter().cloned());
        Some(answer)
    }
}

fn a_real_key(peer: &Rig) -> String {
    let prefix = format!("{}\u{0}", peer.namespace);
    peer.cache
        .resident_pages(&[prefix.as_str()], 10_000)
        .remove(0)
        .into_iter()
        .map(|p| p.key.to_string())
        .find(|k| {
            k.contains("\u{0}p\u{0}")
                && !peer.cache.is_pinned_name(k.split('\u{0}').nth(1).unwrap())
        })
        .expect("a plain page")
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn a_page_the_open_commit_does_not_accept_is_never_asked_for() {
    let a = oss_testkit::TempDir::new("warm-applies-named-peer");
    let b = oss_testkit::TempDir::new("warm-applies-named-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let real = a_real_key(&peer);
    let mut tampered = Tampered::new(&peer);
    tampered.extra_named = vec![
        // A file the commit does not hold.
        ("/idx/0/t=9\u{0}evil\u{0}1\u{0}p\u{0}v1:x".into(), 0, 128),
        // A page past the end of a file it does hold.
        (real.clone(), 9_999, 128),
        // A page of a file it holds at a length the commit does not store.
        (real.clone(), 0, 7),
    ];
    let progress = Progress::default();
    let outcome = copy_from_peer(
        &tampered,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Done);
    let asked = tampered.asked.borrow();
    assert!(!asked.is_empty());
    assert!(!asked.iter().any(|(k, _)| k.contains("evil")));
    assert!(!asked.iter().any(|(_, p)| *p == 9_999));
    assert_eq!(progress.copied(), progress.named());
    assert!(joiner.peek_page(&real, 9_999).is_none());
}

// docs/specs/reader-warm-copy AC-9
#[test]
fn a_page_the_peer_adds_that_was_not_asked_ends_its_copy_and_is_not_inserted() {
    let a = oss_testkit::TempDir::new("warm-applies-served-peer");
    let peer = rig(a.path());
    let real = a_real_key(&peer);
    for (key, page, len) in [
        (
            "/idx/0/t=9\u{0}evil\u{0}1\u{0}p\u{0}v1:x".to_owned(),
            0_u64,
            128_usize,
        ),
        (real.clone(), 9_999, 128),
    ] {
        let b = oss_testkit::TempDir::new("warm-applies-served-joiner");
        let joiner = cache(b.path());
        let mut tampered = Tampered::new(&peer);
        tampered.extra_served = vec![PageBytes {
            key: key.clone(),
            page,
            bytes: vec![7; len],
        }];
        let progress = Progress::default();
        let outcome = copy_from_peer(
            &tampered,
            &Ours(Arc::clone(&peer.expected)),
            &joiner,
            &settings(1 << 20),
            &Sleeps::default(),
            &progress,
            &never,
        );
        assert_eq!(outcome, Outcome::Bad, "{key} page {page}");
        assert!(joiner.peek_page(&key, page).is_none(), "{key} page {page}");
        assert_eq!(progress.named(), progress.copied());
    }
}

/// A peer that notes the clock at each batch and the bytes it had answered
/// before it.
struct Clocked<'a> {
    inner: Fake<'a>,
    now: &'a Cell<Duration>,
    sends: RefCell<Vec<(Duration, u64)>>,
    served: Cell<u64>,
}

impl Peer for Clocked<'_> {
    fn hot_set(&self, request: &HotSetRequest) -> Option<HotSetResponse> {
        self.inner.hot_set(request)
    }

    fn pages(&self, request: &PagesRequest) -> Option<PagesResponse> {
        self.sends
            .borrow_mut()
            .push((self.now.get(), self.served.get()));
        let answer = self.inner.pages(request)?;
        let bytes: usize = answer.pages.iter().map(|p| p.bytes.len()).sum();
        self.served
            .set(self.served.get().saturating_add(bytes as u64));
        Some(answer)
    }
}

/// A pause that moves a fake clock.
struct Ticking<'a>(&'a Cell<Duration>);

impl Pause for Ticking<'_> {
    fn pause(&self, duration: Duration) {
        self.0.set(self.0.get().saturating_add(duration));
    }
}

// docs/specs/reader-warm-copy AC-4
#[test]
fn on_a_fake_clock_no_batch_is_sent_before_the_rate_allows_it_over_a_whole_join() {
    let a = oss_testkit::TempDir::new("warm-applies-clock-peer");
    let b = oss_testkit::TempDir::new("warm-applies-clock-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let now = Cell::new(Duration::ZERO);
    let clocked = Clocked {
        inner: Fake::new(&peer),
        now: &now,
        sends: RefCell::new(Vec::new()),
        served: Cell::new(0),
    };
    let rate = 16 * 1024;
    let progress = Progress::default();
    let outcome = copy_from_peer(
        &clocked,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(rate),
        &Ticking(&now),
        &progress,
        &never,
    );
    assert_eq!(outcome, Outcome::Done);
    let sends = clocked.sends.borrow();
    assert!(sends.len() > 2, "{} batches", sends.len());
    for (at, before) in sends.iter() {
        assert!(
            *at >= pause_for(*before, rate),
            "a batch sent at {at:?} after {before} bytes, early for {rate} B/s"
        );
    }
    let total = progress.copied();
    assert!(now.get() >= pause_for(total, rate));
    // Not slower than the rate needs: the pauses are the bytes over the rate,
    // give or take a nanosecond a batch of rounding.
    assert!(now.get() <= pause_for(total, rate) + Duration::from_micros(sends.len() as u64));
}

// docs/specs/reader-warm-copy AC-6
#[test]
fn a_peer_that_refuses_as_a_java_node_does_leaves_the_other_peers_pages_and_the_node_ready() {
    use crate::lifecycle::Readiness;
    use crate::warm_boot::{Join, finish_when_warm};

    let a = oss_testkit::TempDir::new("warm-applies-java-peer");
    let b = oss_testkit::TempDir::new("warm-applies-java-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let progress = Progress::default();
    let ours = Ours(Arc::clone(&peer.expected));

    // A node that does not know the action answers with an error: no list.
    let java = Fake::new(&peer);
    java.answers.set(false);
    let refused = copy_from_peer(
        &java,
        &ours,
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(refused, Outcome::Unreachable);
    assert_eq!((progress.named(), progress.copied()), (0, 0));
    assert_eq!(java.calls.get(), 0);

    let good = Fake::new(&peer);
    let done = copy_from_peer(
        &good,
        &ours,
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(done, Outcome::Done);
    assert!(progress.copied() > 0);

    let readiness = Readiness::warming();
    let clock = Sleeps::default();
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
        &AtomicBool::new(false),
    );
    assert!(!timed_out);
    assert!(readiness.ready());
    assert_eq!(waited(), Duration::ZERO, "the refusal cost no wait");
}

// docs/specs/reader-warm-copy AC-6, AC-7
#[test]
fn a_peer_that_never_finishes_holds_the_node_to_max_wait_with_the_others_pages_in() {
    use crate::lifecycle::Readiness;
    use crate::warm_boot::{Join, finish_when_warm};

    let a = oss_testkit::TempDir::new("warm-applies-stall-peer");
    let b = oss_testkit::TempDir::new("warm-applies-stall-joiner");
    let peer = rig(a.path());
    let joiner = cache(b.path());
    let progress = Progress::default();
    let good = Fake::new(&peer);
    let done = copy_from_peer(
        &good,
        &Ours(Arc::clone(&peer.expected)),
        &joiner,
        &settings(1 << 20),
        &Sleeps::default(),
        &progress,
        &never,
    );
    assert_eq!(done, Outcome::Done);
    let taken = progress.copied();
    // Another peer named as much again and has given none of it.
    progress.seed(taken * 2, taken);

    let readiness = Readiness::warming();
    let clock = Sleeps::default();
    let waited = || clock.0.borrow().iter().sum::<Duration>();
    let expired = AtomicBool::new(false);
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
    assert!(timed_out);
    assert!(expired.load(std::sync::atomic::Ordering::Acquire));
    assert!(readiness.ready(), "let in at max_wait, not held further");
    assert_eq!(waited(), Duration::from_secs(60));
    let key = a_real_key(&peer);
    assert!(
        joiner.peek_page(&key, 0).is_some(),
        "the good peer's pages are in"
    );
}
