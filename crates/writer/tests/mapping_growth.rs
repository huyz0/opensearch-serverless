//! Growing an index's mapping in its descriptor over the in-process
//! emulator: the swap and its version, the lost swaps and their waits, the
//! refusals, and callers coalescing.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::IndexRecord;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_writer::mapping::Mapping;
use oss_writer::mapping_growth::{GrowthError, MappingGrowth, wait_bound_millis};

const START: &str = r#"{"properties":{"a":{"type":"keyword"}}}"#;

fn mapping(text: &str) -> Mapping {
    Mapping::parse(text.as_bytes()).expect("mapping")
}

fn field(name: &str) -> Mapping {
    mapping(&format!(
        r#"{{"properties":{{"{name}":{{"type":"keyword"}}}}}}"#
    ))
}

struct Rig {
    store: Arc<Emulator>,
    descriptors: Arc<DescriptorStore>,
}

fn rig_on(store: Arc<dyn BlobStore>, emulator: Arc<Emulator>) -> Rig {
    let descriptors =
        Arc::new(DescriptorStore::new(store, "", DeleteMode::Tombstone).expect("descriptors"));
    let text = mapping(START).to_json();
    let mut buf = vec![0u8; 1 << 16];
    descriptors
        .create(
            &IndexRecord {
                mapping: Some(text.into()),
                ..IndexRecord::new("logs", "u1", 1)
            },
            0,
            &mut buf,
        )
        .expect("created");
    Rig {
        store: emulator,
        descriptors,
    }
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 24, 3));
    rig_on(Arc::clone(&store) as Arc<dyn BlobStore>, store)
}

fn described(r: &Rig) -> (String, i64) {
    let mut buf = vec![0u8; 1 << 16];
    let record = r
        .descriptors
        .get("logs", &mut buf)
        .expect("reads")
        .expect("an index");
    (
        record.mapping.as_deref().unwrap_or_default().to_owned(),
        record.mapping_version,
    )
}

type Waits = Arc<Mutex<Vec<Duration>>>;

fn growth(r: &Rig) -> (MappingGrowth, Waits) {
    let waits: Waits = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&waits);
    let g = MappingGrowth::with_sleeper(
        Arc::clone(&r.descriptors),
        Arc::new(move |d| record.lock().expect("lock").push(d)),
    );
    (g, waits)
}

fn swaps(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Put)
}

#[test]
fn growing_writes_the_merged_mapping_and_moves_the_version_by_one() {
    let r = rig();
    let (g, _) = growth(&r);
    let (_, before) = described(&r);
    let grown = g.grow("logs", "u1", &field("b")).expect("grows");
    let (text, after) = described(&r);
    assert_eq!(text, grown.to_json());
    assert!(text.contains("\"a\"") && text.contains("\"b\""), "{text}");
    assert_eq!(after, before + 1);
    let mut buf = vec![0u8; 1 << 16];
    let record = r
        .descriptors
        .get("logs", &mut buf)
        .expect("reads")
        .expect("index");
    assert_eq!((record.uuid.as_ref(), record.number_of_shards), ("u1", 1));
}

#[test]
fn an_addition_the_mapping_already_has_swaps_nothing() {
    let r = rig();
    let (g, waits) = growth(&r);
    let (text, version) = described(&r);
    let puts = swaps(&r);
    let same = g.grow("logs", "u1", &field("a")).expect("grows");
    assert_eq!(same.to_json(), text);
    assert_eq!(described(&r), (text, version));
    assert_eq!(swaps(&r), puts, "no swap");
    assert!(waits.lock().expect("lock").is_empty());
}

#[test]
fn the_waits_follow_the_javas_bounds() {
    let bounds: Vec<u64> = (0..6).map(wait_bound_millis).collect();
    assert_eq!(bounds, [0, 40, 80, 160, 320, 500]);
    assert_eq!(wait_bound_millis(30), 500);
}

/// A store whose register swaps lose `lose` times, each to a rival that
/// adds `rival_field_<n>` first.
fn losing(lose: u32) -> (Rig, Arc<AtomicU64>) {
    let emulator = Arc::new(Emulator::new(1 << 24, 3));
    let lost = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&lost);
    let inner = Arc::clone(&emulator);
    let hooked = common::Hooked::swapping(&emulator, move |e, key, expected, value| {
        let n = count.fetch_add(1, Ordering::SeqCst);
        if n < u64::from(lose) && key.starts_with("indices/") {
            // A rival writer moves the descriptor on before this swap.
            let mut buf = vec![0u8; 1 << 16];
            let rival = DescriptorStore::new(
                Arc::clone(&inner) as Arc<dyn BlobStore>,
                "",
                DeleteMode::Tombstone,
            )
            .expect("rival store");
            let found = rival.resolve("logs", &mut buf).expect("reads");
            let oss_meta::Target::Index(record) = found.target else {
                panic!("an index");
            };
            let merged = Mapping::from_descriptor(record.mapping.as_deref())
                .expect("mapping")
                .merge(&field(&format!("rival_{n}")))
                .expect("merges");
            let mut updated = record.clone();
            updated.mapping = Some(merged.to_json().into());
            updated.mapping_version = updated.mapping_version.saturating_add(1);
            let mut out = vec![0u8; 1 << 16];
            rival
                .update(&updated, found.generation, &mut out)
                .expect("rival updates")
                .expect("rival wins");
        }
        e.swap_register(key, expected, value)
    });
    let r = rig_on(hooked, emulator);
    // The setup's own create is not a swap that counts.
    lost.store(0, Ordering::SeqCst);
    (r, lost)
}

#[test]
fn a_lost_swap_reads_again_so_both_writers_keep_their_fields() {
    let (r, _) = losing(2);
    let (g, waits) = growth(&r);
    let grown = g.grow("logs", "u1", &field("mine")).expect("grows");
    let text = grown.to_json();
    for name in ["a", "mine", "rival_0", "rival_1"] {
        assert!(text.contains(&format!("\"{name}\"")), "{name} in {text}");
    }
    assert_eq!(described(&r).0, text);
    let waits = waits.lock().expect("lock");
    assert_eq!(waits.len(), 2);
    for (i, w) in waits.iter().enumerate() {
        let bound = wait_bound_millis(u32::try_from(i + 1).expect("small"));
        let ms = u64::try_from(w.as_millis()).expect("small");
        assert!((1..=bound).contains(&ms), "wait {i}: {ms} of {bound}");
    }
}

#[test]
fn five_lost_swaps_fail_with_concurrent_after_four_waits() {
    let (r, _) = losing(100);
    let (g, waits) = growth(&r);
    let err = g.grow("logs", "u1", &field("mine")).expect_err("fails");
    assert!(matches!(err, GrowthError::Concurrent(_)), "{err:?}");
    assert!(err.to_string().contains("retry the write"));
    assert_eq!(waits.lock().expect("lock").len(), 4);
}

#[test]
fn a_refused_merge_is_a_conflict_after_one_read_and_poisons_nothing() {
    let r = rig();
    let (g, waits) = growth(&r);
    let puts = swaps(&r);
    let clash = mapping(r#"{"properties":{"a":{"type":"long"}}}"#);
    let err = g.grow("logs", "u1", &clash).expect_err("refused");
    assert!(matches!(err, GrowthError::Conflict(_)), "{err:?}");
    assert!(
        err.to_string().contains("cannot be changed from type"),
        "{err}"
    );
    assert_eq!(swaps(&r), puts, "no swap");
    assert!(waits.lock().expect("lock").is_empty(), "no wait");
    g.grow("logs", "u1", &field("b"))
        .expect("a later growth works");
}

#[test]
fn an_index_that_is_gone_or_recreated_is_deleted() {
    let r = rig();
    let (g, _) = growth(&r);
    let puts = swaps(&r);
    let other = g.grow("logs", "other-uuid", &field("b")).expect_err("uuid");
    assert!(matches!(other, GrowthError::Deleted(_)), "{other:?}");
    let absent = g.grow("nope", "u1", &field("b")).expect_err("absent");
    assert!(matches!(absent, GrowthError::Deleted(_)), "{absent:?}");
    r.descriptors.delete("logs").expect("deleted");
    let gone = g.grow("logs", "u1", &field("b")).expect_err("gone");
    assert!(matches!(gone, GrowthError::Deleted(_)), "{gone:?}");
    assert_eq!(swaps(&r), puts, "no swap");
}

#[test]
fn concurrent_callers_coalesce_and_each_keeps_its_field() {
    let r = rig();
    let g = Arc::new(MappingGrowth::new(Arc::clone(&r.descriptors)));
    let n = 16usize;
    let start = Arc::new(Barrier::new(n));
    let puts = swaps(&r);
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let g = Arc::clone(&g);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                g.grow("logs", "u1", &field(&format!("f{i}")))
                    .expect("grows")
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }
    let (text, version) = described(&r);
    for i in 0..n {
        assert!(text.contains(&format!("\"f{i}\"")), "f{i} in {text}");
    }
    assert!(version <= 1 + i64::try_from(n).expect("small"));
    assert!(swaps(&r) - puts < u64::try_from(n).expect("small") * 2);
}

#[test]
fn an_incompatible_addition_in_a_batch_refuses_only_its_caller() {
    let r = rig();
    let g = Arc::new(MappingGrowth::new(Arc::clone(&r.descriptors)));
    let start = Arc::new(Barrier::new(2));
    let run = |m: Mapping| {
        let g = Arc::clone(&g);
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            g.grow("logs", "u1", &m)
        })
    };
    let good = run(field("fine"));
    let bad = run(mapping(r#"{"properties":{"a":{"type":"long"}}}"#));
    assert!(good.join().expect("thread").is_ok());
    assert!(matches!(
        bad.join().expect("thread"),
        Err(GrowthError::Conflict(_))
    ));
    assert!(described(&r).0.contains("\"fine\""));
}

#[test]
fn a_batch_the_store_failed_goes_back_to_the_callers_waiting_on_it() {
    let emulator = Arc::new(Emulator::new(1 << 24, 3));
    let leader_in = Arc::new(AtomicU64::new(0));
    let follower_ready = Arc::new(AtomicU64::new(0));
    let (inside, ready) = (Arc::clone(&leader_in), Arc::clone(&follower_ready));
    let first = Arc::new(AtomicU64::new(1));
    let hooked = common::Hooked::swapping(&emulator, move |e, key, expected, value| {
        if first.swap(0, Ordering::SeqCst) == 1 {
            // The leader is mid swap: let the follower queue behind it, then fail.
            inside.store(1, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while ready.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(100));
            return Err(oss_blob::BlobError::Stream("the store failed"));
        }
        e.swap_register(key, expected, value)
    });
    let r = rig_on(hooked, emulator);
    let g = Arc::new(MappingGrowth::new(Arc::clone(&r.descriptors)));
    let leader = {
        let g = Arc::clone(&g);
        std::thread::spawn(move || g.grow("logs", "u1", &field("lost")))
    };
    while leader_in.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    let follower = {
        let g = Arc::clone(&g);
        let ready = Arc::clone(&follower_ready);
        std::thread::spawn(move || {
            ready.store(1, Ordering::SeqCst);
            g.grow("logs", "u1", &field("next"))
        })
    };
    let err = leader.join().expect("thread").expect_err("store fails");
    assert!(matches!(err, GrowthError::Store(_)), "{err:?}");
    let grown = follower.join().expect("thread").expect("grows");
    for name in ["a", "lost", "next"] {
        assert!(grown.to_json().contains(&format!("\"{name}\"")), "{name}");
    }
    assert_eq!(described(&r).0, grown.to_json());
}

#[test]
fn a_store_failure_with_nobody_waiting_leaves_the_mapping_alone() {
    let emulator = Arc::new(Emulator::new(1 << 24, 3));
    let fail = Arc::new(AtomicU64::new(1));
    let armed = Arc::clone(&fail);
    let hooked = common::Hooked::swapping(&emulator, move |e, key, expected, value| {
        if armed.swap(0, Ordering::SeqCst) == 1 {
            return Err(oss_blob::BlobError::Stream("the store failed"));
        }
        e.swap_register(key, expected, value)
    });
    let r = rig_on(hooked, emulator);
    let (g, _) = growth(&r);
    let err = g
        .grow("logs", "u1", &field("lost"))
        .expect_err("store fails");
    assert!(matches!(err, GrowthError::Store(_)), "{err:?}");
    assert!(!described(&r).0.contains("\"lost\""));
    let grown = g.grow("logs", "u1", &field("next")).expect("grows");
    assert!(
        !grown.to_json().contains("\"lost\""),
        "a failed request's addition is not kept"
    );
}
