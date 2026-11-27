//! The one-time seeding walk of `packs/`: a node that left before the first
//! collector ran is found by its ledger and named in the index, once a
//! cluster, resumably, and never again after it is done.

use crate::common::Hooked;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oss_blob::{BlobStore, Emulator, S3Op, SliceSource};
use oss_format::{PackSeed, pack_seed_key};
use oss_writer::pack::{PackIndexStore, PackLedger, PackSeeder, Seeded};

fn emulator() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 30, 7))
}

fn dynstore(e: &Arc<Emulator>) -> Arc<dyn BlobStore> {
    Arc::clone(e) as Arc<dyn BlobStore>
}

/// A node with a live epoch in its ledger.
fn live(e: &Arc<Emulator>, node: &str) {
    let ledger = PackLedger::new(dynstore(e), "", node).expect("ledger");
    ledger.add_epoch(1).expect("epoch");
    ledger.reserve(1, 4).expect("high");
}

/// A node whose ledger holds no epoch, and one with a pack but no ledger.
fn gone(e: &Arc<Emulator>, node: &str) {
    e.put(
        &format!("packs/{node}/1-0"),
        &mut SliceSource::new(b"pack"),
        &mut [0; 16],
    )
    .expect("put");
}

fn named(e: &Arc<Emulator>, node: &str) -> bool {
    let index = PackIndexStore::new(dynstore(e), "");
    let slot = oss_format::pack_index_slot(node);
    index.read(slot).expect("read").index.contains(node)
}

fn seed_state(e: &Arc<Emulator>) -> PackSeed {
    let mut buf = [0u8; 4096];
    let read = e
        .read_register(&pack_seed_key(""), &mut buf)
        .expect("read")
        .expect("exists");
    PackSeed::parse(&buf[..read.len]).expect("parses")
}

fn seeder(e: &Arc<Emulator>, page: usize) -> PackSeeder {
    PackSeeder::new(dynstore(e), "", page)
}

/// Nodes with a live epoch are named; one with a pack but no ledger and the
/// index's own registers are not.
#[test]
fn only_nodes_with_a_live_epoch_are_named() {
    let e = emulator();
    live(&e, "java-1");
    live(&e, "java-3");
    gone(&e, "java-2");
    PackIndexStore::new(dynstore(&e), "")
        .add("rust-1", 1)
        .expect("a register under packs/index/");
    let step = seeder(&e, 100).step(500).expect("step");
    assert_eq!(
        step,
        Seeded::Page {
            named: 2,
            done: true
        }
    );
    assert!(named(&e, "java-1") && named(&e, "java-3"));
    assert!(!named(&e, "java-2"));
    assert!(!named(&e, "index"));
    assert!(seed_state(&e).done());
}

/// A done seed costs no LIST on any later step, from this seeder or a new one
/// after a restart.
#[test]
fn a_done_seed_issues_no_list() {
    let e = emulator();
    live(&e, "java-1");
    seeder(&e, 100).step(500).expect("seeds");
    let lists = e.meter().count(S3Op::List);
    assert_eq!(seeder(&e, 100).step(900).expect("again"), Seeded::Done);
    assert_eq!(seeder(&e, 100).step(900).expect("restarted"), Seeded::Done);
    assert_eq!(e.meter().count(S3Op::List), lists);
}

/// A page of two children over five nodes takes four steps, each a bounded
/// number of LISTs, and a seeder made after the first step resumes from the
/// cursor and never lists a child already read.
#[test]
fn a_crash_mid_seed_resumes_from_the_cursor() {
    let e = emulator();
    for n in ["a", "b", "c", "d", "e"] {
        live(&e, n);
    }
    let first = seeder(&e, 2).step(1).expect("first page");
    assert_eq!(
        first,
        Seeded::Page {
            named: 2,
            done: false
        }
    );
    assert_eq!(seed_state(&e).cursor(), Some("packs/b/"));
    let lists = e.meter().count(S3Op::List);
    let second = seeder(&e, 2).step(2).expect("second page");
    assert_eq!(
        second,
        Seeded::Page {
            named: 2,
            done: false
        }
    );
    // Two children, one LIST each, plus none that re-reads a or b.
    assert_eq!(e.meter().count(S3Op::List) - lists, 2);
    // The page holding `e` and the index's own child is full, so the end is
    // found by one more step that reads nothing.
    let third = seeder(&e, 2).step(3).expect("third page");
    assert_eq!(
        third,
        Seeded::Page {
            named: 1,
            done: false
        }
    );
    let last = seeder(&e, 2).step(4).expect("the end");
    assert_eq!(
        last,
        Seeded::Page {
            named: 0,
            done: true
        }
    );
    assert!(seed_state(&e).done());
    for n in ["a", "b", "c", "d", "e"] {
        assert!(named(&e, n), "{n}");
    }
}

/// A seeder that reads a page and then finds the cursor moved under it stops:
/// the other one's page stands, and this one's cursor is not written.
#[test]
fn a_racing_seeder_loses_the_swap_and_stops() {
    let e = emulator();
    for n in ["a", "b", "c", "d"] {
        live(&e, n);
    }
    seeder(&e, 2).step(1).expect("first page");
    let once = Arc::new(AtomicBool::new(false));
    let racing = Arc::clone(&once);
    let store = Hooked::swapping(&e, move |inner, key, expected, value| {
        if key.ends_with("packs/index/seeded") && !racing.swap(true, Ordering::SeqCst) {
            // A rival finishes the page first.
            let rival_state = PackSeed::after("packs/d/").encode().expect("encodes");
            inner.swap_register(key, expected, &rival_state)?;
        }
        inner.swap_register(key, expected, value)
    });
    let out = PackSeeder::new(store, "", 2).step(2).expect("step");
    assert_eq!(out, Seeded::Lost);
    assert_eq!(seed_state(&e).cursor(), Some("packs/d/"));
}

/// Two first collectors that find no register: one begins it, the other
/// loses the create and stops without listing.
#[test]
fn a_second_first_collector_stops_without_listing() {
    let e = emulator();
    live(&e, "a");
    let store = Hooked::swapping(&e, |inner, key, expected, value| {
        if key.ends_with("packs/index/seeded") {
            // A rival begins the seed between this one's read and its create.
            inner.create_register(key, &PackSeed::begun().encode().expect("encodes"))?;
        }
        inner.swap_register(key, expected, value)
    });
    let lists = e.meter().count(S3Op::List);
    let out = PackSeeder::new(store, "", 10).step(1).expect("step");
    assert_eq!(out, Seeded::Lost);
    assert_eq!(e.meter().count(S3Op::List), lists);
    assert!(!seed_state(&e).done());
}
