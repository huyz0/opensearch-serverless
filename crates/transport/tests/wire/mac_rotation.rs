//! The request MAC across secret generations: the previous key after a
//! rotation, and refreshing the secret for a generation not yet held.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_transport::auth::{AuthConfig, Authenticator, SecretSource, Secrets};
use oss_transport::exception::ExceptionKind;

use crate::macs::{K0, K1, NOW, ROTATED, Source, WINDOW, auth, clock, holding, secrets, status};

#[test]
fn the_previous_key_verifies_for_one_window_after_a_rotation() {
    let old = holding(Some(secrets(2, K0, None)));
    let now = clock(NOW);
    let header = auth("s/1", &old, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    let rotated = holding(Some(secrets(3, K1, Some(K0))));
    auth("r/1", &rotated, &now, 16)
        .verify("act", b"x", Some(&header), &[])
        .unwrap();
    // Past one window since the rotation, the previous key is refused.
    let late = clock(ROTATED + WINDOW + 1);
    let header = auth("s/1", &old, &late, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    assert_eq!(
        status(auth("r/1", &rotated, &late, 16).verify("act", b"x", Some(&header), &[])),
        403
    );
    // Two generations back, or with no previous key, is refused.
    let older = holding(Some(secrets(1, K0, None)));
    let header = auth("s/1", &older, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    assert_eq!(
        status(auth("r/1", &rotated, &now, 16).verify("act", b"x", Some(&header), &[])),
        403
    );
    let header = auth("s/1", &old, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    let no_previous = holding(Some(secrets(3, K1, None)));
    assert_eq!(
        status(auth("r/1", &no_previous, &now, 16).verify("act", b"x", Some(&header), &[])),
        403
    );
}

#[test]
fn a_newer_generation_refreshes_once_a_second_and_rejects_meanwhile() {
    let now = clock(NOW);
    let newer = holding(Some(secrets(4, K0, Some(K1))));
    let header = auth("s/1", &newer, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    // The receiver still holds generation 3; its refresh finds 4.
    let stale = holding(Some(secrets(3, K1, None)));
    *stale.next.lock().unwrap() = Some(secrets(4, K0, Some(K1)));
    let receiver = auth("r/1", &stale, &now, 16);
    receiver.verify("act", b"x", Some(&header), &[]).unwrap();
    assert_eq!(stale.refreshes.load(Ordering::SeqCst), 1);
    // A generation no refresh finds: the first probe refreshes and refuses,
    // the next within a second is rejected (429) without a refresh.
    let bogus = format!("9:s/1:{NOW}:00:00");
    // A fresh receiver: the first one probed under a second ago.
    let receiver = auth("r/1", &stale, &now, 16);
    assert_eq!(status(receiver.verify("act", b"x", Some(&bogus), &[])), 403);
    assert_eq!(stale.refreshes.load(Ordering::SeqCst), 2);
    let rejected = receiver.verify("act", b"x", Some(&bogus), &[]).unwrap_err();
    assert_eq!(rejected.kind, Some(ExceptionKind::Jdk(18)));
    assert_eq!(rejected.http_status(), 429);
    assert_eq!(stale.refreshes.load(Ordering::SeqCst), 2);
    // A refresh that leaves the node with no secret refuses.
    let lost = holding(Some(secrets(3, K1, None)));
    let receiver = auth("r/1", &lost, &now, 16);
    *lost.secrets.lock().unwrap() = Some(secrets(3, K1, None));
    struct Vanish(Arc<Source>);
    impl SecretSource for Vanish {
        fn secrets(&self) -> Option<Arc<Secrets>> {
            self.0.secrets()
        }
        fn refresh(&self) {
            *self.0.secrets.lock().unwrap() = None;
        }
    }
    let vanishing = Authenticator::new(
        AuthConfig {
            identity: "r/1".into(),
            window_millis: WINDOW,
            replay_capacity: 4,
        },
        Arc::new(Vanish(Arc::clone(&lost))),
        Arc::new(move || NOW),
    );
    assert_eq!(
        status(vanishing.verify("act", b"x", Some(&bogus), &[])),
        403
    );
    drop(receiver);
}

#[test]
fn a_header_at_the_current_generation_never_refreshes() {
    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let header = auth("s/1", &source, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    auth("r/1", &source, &now, 16)
        .verify("act", b"x", Some(&header), &[])
        .unwrap();
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 0);
}

/// Holds generation 3 for its first read and 4 after, as when another
/// caller's refresh lands between two reads of one verify.
struct Landing {
    reads: AtomicUsize,
}

impl SecretSource for Landing {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        Some(if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            secrets(3, K1, None)
        } else {
            secrets(4, K0, Some(K1))
        })
    }
    fn refresh(&self) {}
}

#[test]
fn a_generation_another_refresh_installed_meanwhile_verifies_without_a_probe() {
    let now = clock(NOW);
    let newer = holding(Some(secrets(4, K0, Some(K1))));
    let sign = |nonce_from: &Authenticator| nonce_from.sign("act", b"x", "r/1", &[]).unwrap();
    let signer = auth("s/1", &newer, &now, 16);
    let landing = Arc::new(Landing {
        reads: AtomicUsize::new(1),
    });
    let receiver = Authenticator::new(
        AuthConfig {
            identity: "r/1".into(),
            window_millis: WINDOW,
            replay_capacity: 4,
        },
        Arc::clone(&landing) as Arc<dyn SecretSource>,
        Arc::new(move || NOW),
    );
    // Use up the probe: generation 9 is newer than anything held.
    let _ = receiver.verify("act", b"x", Some(&format!("9:s/1:{NOW}:00:00")), &[]);
    // The next verify reads 3 first and 4 after: the probe is spent, but
    // the generation is held by then, so it verifies rather than waits.
    landing.reads.store(0, Ordering::SeqCst);
    receiver
        .verify("act", b"x", Some(&sign(&signer)), &[])
        .unwrap();
}

#[test]
fn a_probe_once_finished_allows_another_a_second_later() {
    let now = clock(NOW);
    let stale = holding(Some(secrets(3, K1, None)));
    let receiver = auth("r/1", &stale, &now, 16);
    let bogus = format!("9:s/1:{NOW}:00:00");
    assert_eq!(status(receiver.verify("act", b"x", Some(&bogus), &[])), 403);
    assert_eq!(stale.refreshes.load(Ordering::SeqCst), 1);
    std::thread::sleep(std::time::Duration::from_millis(1_050));
    assert_eq!(status(receiver.verify("act", b"x", Some(&bogus), &[])), 403);
    assert_eq!(stale.refreshes.load(Ordering::SeqCst), 2);
}
