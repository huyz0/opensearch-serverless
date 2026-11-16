//! The transport secret register (docs/specs/transport/SPEC.md, AC-13):
//! a Java-written value read at the register's generation, the first
//! node's mint, a lost mint, and the cache's one read a lease TTL.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::LeaseClock;
use oss_cluster::transport_secret::{CachedTransportSecrets, TransportSecretStore};
use oss_format::transport_secret_key;
use oss_transport::auth::{AuthConfig, Authenticator, SecretSource};

use crate::{BASE, Fleet, Racing, T0, TTL};

/// `fixtures/transport-secret/written.txt`'s second value, as the Java wrote it.
const JAVA_VALUE: &[u8] =
    br#"{"generation":42,"current":"aa11","previous":"bb22","rotated_at":1700000000123}"#;

fn store_on(fleet: &Fleet, store: Arc<dyn BlobStore>) -> TransportSecretStore {
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    TransportSecretStore::new(store, BASE, clock).unwrap()
}

fn key() -> String {
    transport_secret_key(BASE).unwrap()
}

#[test]
fn a_java_written_value_reads_at_the_registers_generation_in_one_get() {
    let fleet = Fleet::new();
    let generation = match fleet.store.create_register(&key(), JAVA_VALUE).unwrap() {
        oss_blob::SwapOutcome::Applied(g) => g,
        other => panic!("{other:?}"),
    };
    let before = fleet.total();
    let secrets = store_on(&fleet, fleet.store.clone()).read().unwrap();
    assert_eq!(fleet.total() - before, 1);
    assert_eq!(fleet.count(S3Op::Put), 1);
    assert_eq!(secrets.generation, generation.cast_signed());
    assert_ne!(secrets.generation, 42);
    assert_eq!(secrets.current, "aa11");
    assert_eq!(secrets.previous.as_deref(), Some("bb22"));
    assert_eq!(secrets.rotated_at_millis, 1_700_000_000_123);
}

#[test]
fn the_first_node_mints_a_fresh_key_that_every_later_read_shares() {
    let fleet = Fleet::new();
    let store = store_on(&fleet, fleet.store.clone());
    let minted = store.read().unwrap();
    // One read finding nothing, one conditional create.
    assert_eq!(fleet.total(), 2);
    assert_eq!(fleet.count(S3Op::Put), 1);
    assert_eq!(minted.current.len(), 64);
    assert!(
        minted
            .current
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(minted.previous, None);
    assert_eq!(minted.rotated_at_millis, T0);
    let again = store_on(&fleet, fleet.store.clone()).read().unwrap();
    assert_eq!(fleet.total(), 3);
    assert_eq!(again.current, minted.current);
    assert_eq!(again.generation, minted.generation);
    let other = store_on(&fleet, Arc::new(oss_blob::Emulator::new(1 << 20, 2)))
        .read()
        .unwrap();
    assert_ne!(other.current, minted.current, "keys are random");
}

#[test]
fn a_node_whose_mint_loses_reads_the_winners_key() {
    let fleet = Fleet::new();
    let racing = Racing::new(fleet.store.clone());
    let inner = fleet.store.clone();
    racing.set(move |key| {
        let _ = inner.create_register(key, JAVA_VALUE).unwrap();
    });
    let secrets = store_on(&fleet, racing).read().unwrap();
    assert_eq!(secrets.current, "aa11");
    assert_eq!(secrets.previous.as_deref(), Some("bb22"));
}

#[test]
fn a_mint_whose_winner_vanished_is_an_error() {
    let fleet = Fleet::new();
    let racing = Racing::new(fleet.store.clone());
    let inner = fleet.store.clone();
    let reads = Arc::new(AtomicU32::new(0));
    let seen = reads.clone();
    // The winner's create lands before ours; then, before our read after
    // the lost create, the register is deleted.
    racing.set({
        let inner = inner.clone();
        move |key| {
            let _ = inner.create_register(key, JAVA_VALUE).unwrap();
        }
    });
    *racing.read_hook.lock().unwrap() = Some(Arc::new(move |key: &str| {
        if seen.fetch_add(1, Ordering::SeqCst) == 1 {
            let mut buf = [0u8; 256];
            let g = inner
                .read_register(key, &mut buf)
                .unwrap()
                .unwrap()
                .generation;
            assert!(inner.delete_register(key, g).unwrap());
        }
    }));
    let err = store_on(&fleet, racing).read().unwrap_err();
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        err.to_string(),
        "the transport secret vanished after a lost create"
    );
}

#[test]
fn a_malformed_value_is_an_error_and_the_cache_holds_nothing() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), b"   ").unwrap();
    assert!(store_on(&fleet, fleet.store.clone()).read().is_err());
    let cache = CachedTransportSecrets::new(store_on(&fleet, fleet.store.clone()), TTL);
    assert!(cache.secrets().is_none());
}

#[test]
fn the_cache_reads_once_a_lease_ttl_or_when_asked() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), JAVA_VALUE).unwrap();
    let cache = CachedTransportSecrets::new(store_on(&fleet, fleet.store.clone()), TTL);
    let first = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 1);
    for _ in 0..100 {
        assert_eq!(cache.secrets().unwrap().current, "aa11");
    }
    fleet.clock.advance(TTL);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 1, "still within one TTL");
    fleet.clock.advance(1);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 2, "past one TTL");
    cache.refresh();
    assert_eq!(fleet.count(S3Op::Get), 3, "asked for");
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 3);
    fleet.clock.advance(-1);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 4, "the clock went back");
    assert_eq!(cache.secrets().unwrap().generation, first.generation);
}

#[test]
fn a_failed_reread_keeps_the_secrets_held() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), JAVA_VALUE).unwrap();
    let cache = CachedTransportSecrets::new(store_on(&fleet, fleet.store.clone()), TTL);
    let held = cache.secrets().unwrap();
    fleet.store.fail_next(S3Op::Get, 1);
    cache.refresh();
    assert_eq!(fleet.count(S3Op::Get), 2);
    for _ in 0..100 {
        assert_eq!(cache.secrets().unwrap().current, held.current);
    }
    assert_eq!(
        fleet.count(S3Op::Get),
        2,
        "a failed read is retried after a second, not per call"
    );
    fleet.clock.advance(1_000);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 2);
    fleet.clock.advance(1);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 3, "retried");
    fleet.clock.advance(TTL);
    let _ = cache.secrets().unwrap();
    assert_eq!(
        fleet.count(S3Op::Get),
        3,
        "a good read is due one TTL later"
    );
    fleet.clock.advance(1);
    let _ = cache.secrets().unwrap();
    assert_eq!(fleet.count(S3Op::Get), 4);
}

#[test]
fn a_store_that_never_answers_is_tried_once_a_second_with_nothing_held() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), JAVA_VALUE).unwrap();
    let cache = CachedTransportSecrets::new(store_on(&fleet, fleet.store.clone()), TTL);
    fleet.store.fail_next(S3Op::Get, 1);
    for _ in 0..10 {
        assert!(cache.secrets().is_none());
    }
    assert_eq!(fleet.count(S3Op::Get), 1);
    fleet.clock.advance(1_001);
    assert_eq!(cache.secrets().unwrap().current, "aa11");
}

/// A one-shot signal waited on with a timeout, so a broken cache fails a
/// test rather than hanging it.
#[derive(Default)]
struct Signal {
    set: Mutex<bool>,
    changed: Condvar,
}

impl Signal {
    fn set(&self) {
        *self.set.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self) -> bool {
        let guard = self.set.lock().unwrap();
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, Duration::from_secs(5), |set| !*set)
            .unwrap();
        *guard
    }
}

#[test]
fn callers_go_on_with_the_held_secrets_while_one_rereads() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), JAVA_VALUE).unwrap();
    let racing = Racing::new(fleet.store.clone());
    let cache = Arc::new(CachedTransportSecrets::new(
        store_on(&fleet, racing.clone()),
        TTL,
    ));
    let held = cache.secrets().unwrap();
    let entered = Arc::new(Signal::default());
    let release = Arc::new(Signal::default());
    let released = Arc::new(AtomicBool::new(false));
    {
        let (entered, release, released) = (entered.clone(), release.clone(), released.clone());
        *racing.read_hook.lock().unwrap() = Some(Arc::new(move |_: &str| {
            entered.set();
            // False when the wait timed out: the other caller blocked.
            released.store(release.wait(), Ordering::SeqCst);
        }));
    }
    let reader = {
        let cache = cache.clone();
        thread::spawn(move || cache.refresh())
    };
    assert!(entered.wait(), "the refresh reached the store");
    // The reader is inside the store; another call is due a read too.
    fleet.clock.advance(TTL + 1);
    let during = cache.secrets();
    *racing.read_hook.lock().unwrap() = None;
    release.set();
    reader.join().unwrap();
    assert!(
        released.load(Ordering::SeqCst),
        "the other caller went on while the reader was in the store"
    );
    assert_eq!(during.unwrap().current, held.current);
    assert_eq!(fleet.count(S3Op::Get), 2);
}

#[test]
fn a_caller_waits_for_the_first_read_and_shares_it() {
    let fleet = Fleet::new();
    let _ = fleet.store.create_register(&key(), JAVA_VALUE).unwrap();
    let racing = Racing::new(fleet.store.clone());
    let cache = Arc::new(CachedTransportSecrets::new(
        store_on(&fleet, racing.clone()),
        TTL,
    ));
    let entered = Arc::new(Signal::default());
    let gate = Arc::new(AtomicU32::new(0));
    {
        let (entered, gate) = (entered.clone(), gate.clone());
        *racing.read_hook.lock().unwrap() = Some(Arc::new(move |_: &str| {
            if gate.fetch_add(1, Ordering::SeqCst) == 0 {
                entered.set();
                // Time for the second caller to reach the lock; a late one
                // takes the fresh path and still shares the one read, so
                // a slow machine can make this pass weakly, never fail.
                thread::sleep(Duration::from_millis(100));
            }
        }));
    }
    let first = {
        let cache = cache.clone();
        thread::spawn(move || cache.secrets())
    };
    assert!(entered.wait(), "the first caller reached the store");
    let second = cache.secrets().unwrap();
    assert_eq!(first.join().unwrap().unwrap().current, second.current);
    assert_eq!(fleet.count(S3Op::Get), 1, "the waiter took the first read");
}

#[test]
fn a_base_that_does_not_end_in_a_slash_is_refused() {
    let fleet = Fleet::new();
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    assert!(TransportSecretStore::new(fleet.store.clone(), "d", clock).is_err());
}

fn authenticator(fleet: &Fleet, store: Arc<dyn BlobStore>, who: &str) -> Authenticator {
    let cache: Arc<dyn SecretSource> =
        Arc::new(CachedTransportSecrets::new(store_on(fleet, store), TTL));
    let clock = fleet.clock.clone();
    Authenticator::new(
        AuthConfig {
            identity: who.into(),
            window_millis: TTL,
            replay_capacity: 16,
        },
        cache,
        Arc::new(move || clock.now()),
    )
}

#[test]
fn two_nodes_on_one_register_verify_each_other_and_a_node_on_another_does_not() {
    let fleet = Fleet::new();
    let a = authenticator(&fleet, fleet.store.clone(), "a/1");
    let b = authenticator(&fleet, fleet.store.clone(), "b/1");
    let stranger = authenticator(&fleet, Arc::new(oss_blob::Emulator::new(1 << 20, 3)), "a/1");
    let request = b"forwarded write";
    let header = a
        .sign("internal:serverless/document/write", request, "b/1", &[])
        .unwrap();
    // The first node minted the key; the second read it.
    assert_eq!(fleet.count(S3Op::Put), 1);
    b.verify(
        "internal:serverless/document/write",
        request,
        Some(&header),
        &[],
    )
    .unwrap();
    let forged = stranger
        .sign("internal:serverless/document/write", request, "b/1", &[])
        .unwrap();
    assert!(
        b.verify(
            "internal:serverless/document/write",
            request,
            Some(&forged),
            &[]
        )
        .is_err()
    );
}
