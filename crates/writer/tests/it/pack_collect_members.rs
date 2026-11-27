//! The collector names the live members no index names and keeps their
//! cost down: a member with no ledger is looked at once an hour, a live node
//! with nothing to judge costs its ledger GET alone, and a walk of `packs/`
//! finds the nodes that left before any collector ran.

use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_writer::pack::{PackCollector, PackIndexStore, WINDOW_MILLIS, collector_for};

use crate::pack_collect_sync::{
    MIN, NODE, Nothing, dynstore, emulator, exists, ledger, members, pack, run, run_as,
    writer_with_ledger,
};

/// A node in no index register, with only a ledger, as a node that left
/// before the first Rust collector ran.
fn unindexed_ledger(e: &Arc<Emulator>, epoch: u64, high: u64) {
    ledger(e).reserve(epoch, high).expect("reserved");
}

/// With seeding on, the walk names a node that is in no index register and
/// its packs are then judged; the walk runs once and a later pass lists
/// nothing.
#[test]
fn a_seeding_collector_finds_a_node_the_index_never_named() {
    let e = emulator();
    unindexed_ledger(&e, 1, 8);
    pack(&e, 1, 0);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100).with_seeding(16);
    run(&mut c, 100, &refs);
    run(&mut c, 101, &refs);
    assert!(!exists(&e, 1, 0), "found by the walk and judged");
    let lists = e.meter().count(S3Op::List);
    run(&mut c, 102, &refs);
    assert_eq!(
        e.meter().count(S3Op::List),
        lists,
        "the walk is not repeated"
    );
}

/// Without seeding an unindexed node is never found.
#[test]
fn a_collector_without_seeding_does_not_look_for_unindexed_nodes() {
    let e = emulator();
    unindexed_ledger(&e, 1, 8);
    pack(&e, 1, 0);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run(&mut c, 100, &refs);
    run(&mut c, 101, &refs);
    assert!(exists(&e, 1, 0));
    assert_eq!(e.meter().count(S3Op::List), 0);
}

/// A walk over several pages takes one page a pass and judges nothing it
/// has not named yet.
#[test]
fn a_walk_over_many_pages_takes_one_page_a_pass() {
    let e = emulator();
    unindexed_ledger(&e, 1, 8);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100).with_seeding(1);
    let mut steps = 0;
    for w in 100..110 {
        run(&mut c, w, &refs);
        if e.meter().count(S3Op::List) > steps {
            steps = e.meter().count(S3Op::List);
        }
    }
    assert!(steps >= 1);
    let after = e.meter().count(S3Op::List);
    run(&mut c, 110, &refs);
    assert_eq!(e.meter().count(S3Op::List), after, "done, so no more LISTs");
}

/// The index register that would name `NODE`.
fn index_state(e: &Arc<Emulator>) -> oss_writer::pack::IndexState {
    PackIndexStore::new(dynstore(e), "")
        .read(oss_format::pack_index_slot(NODE))
        .expect("index")
}

/// The seeding walk already finished, so only first sight can name a node.
fn seeding_done(e: &Arc<Emulator>) {
    let key = oss_format::pack_seed_key("");
    let bytes = oss_format::PackSeed::finished().encode().expect("encode");
    e.swap_register(&key, oss_blob::register::ABSENT_GENERATION, &bytes)
        .expect("swap");
}

/// With seeding on, a live member no register names is added at first sight
/// (a Java node never adds itself) and judged from the next pass.
#[test]
fn a_live_member_no_index_names_is_added_and_then_judged() {
    let e = emulator();
    seeding_done(&e);
    unindexed_ledger(&e, 1, 16);
    pack(&e, 1, 0);
    e.advance_clock(16 * MIN);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100).with_seeding(16);
    run_as(&mut c, 100, &refs, true);
    assert!(index_state(&e).index.contains(NODE), "named at first sight");
    assert!(exists(&e, 1, 0), "not judged in the pass that named it");
    run_as(&mut c, 101, &refs, true);
    run_as(&mut c, 102, &refs, true);
    assert!(!exists(&e, 1, 0), "judged once named");
}

/// A member already named costs no register write, however many passes run.
#[test]
fn unchanged_membership_writes_no_index_register() {
    let e = emulator();
    seeding_done(&e);
    unindexed_ledger(&e, 1, 16);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100).with_seeding(16);
    run_as(&mut c, 100, &refs, true);
    let generation = index_state(&e).generation;
    for w in 101..110 {
        run_as(&mut c, w, &refs, true);
    }
    assert_eq!(index_state(&e).generation, generation);
}

/// Without seeding no member is added to the index.
#[test]
fn a_collector_without_seeding_adds_no_member() {
    let e = emulator();
    unindexed_ledger(&e, 1, 16);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_as(&mut c, 100, &refs, true);
    assert!(!index_state(&e).index.contains(NODE));
}

/// A member id the index cannot hold is skipped and the pass goes on.
#[test]
fn a_member_with_an_unusable_id_is_skipped() {
    let e = emulator();
    seeding_done(&e);
    unindexed_ledger(&e, 1, 16);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100).with_seeding(16);
    let w = 100;
    let now = w * WINDOW_MILLIS + MIN;
    let mut all = members(true);
    all.push("bad id/".to_owned());
    all.sort();
    let me = collector_for(w, &all).expect("elected").to_owned();
    c.tick(&me, &all, now, &refs).expect("tick");
    assert!(index_state(&e).index.contains(NODE));
}

/// Whole-object GETs so far.
fn gets(e: &Arc<Emulator>) -> u64 {
    e.meter().count(S3Op::Get)
}

/// A live member with no ledger costs one GET and is not looked at again for
/// an hour: the passes between ask only the index.
#[test]
fn a_live_member_with_no_ledger_is_looked_at_once_an_hour() {
    let e = emulator();
    PackIndexStore::new(dynstore(&e), "")
        .add(NODE, 1)
        .expect("indexed");
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    let before = gets(&e);
    run_as(&mut c, 100, &refs, true);
    assert_eq!(gets(&e) - before, 16 + 1, "16 index registers, one ledger");
    let before = gets(&e);
    for w in 101..106 {
        run_as(&mut c, w, &refs, true);
    }
    assert_eq!(gets(&e) - before, 5 * 16, "only the index, for five passes");
    let before = gets(&e);
    run_as(&mut c, 106, &refs, true);
    assert_eq!(gets(&e) - before, 16 + 1, "an hour on, the ledger again");
}

/// A ledger that appears is found at the next look.
#[test]
fn a_ledger_that_appears_is_judged_at_the_next_look() {
    let e = emulator();
    PackIndexStore::new(dynstore(&e), "")
        .add(NODE, 1)
        .expect("indexed");
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_as(&mut c, 100, &refs, true);
    ledger(&e).reserve(1, 16).expect("reserved");
    pack(&e, 1, 0);
    e.advance_clock(16 * MIN);
    for w in 101..109 {
        run_as(&mut c, w, &refs, true);
    }
    assert!(!exists(&e, 1, 0), "judged after the hour, not never");
}

/// A dead member with no ledger is not waited on: it leaves the index.
#[test]
fn a_dead_member_with_no_ledger_leaves_the_index_at_once() {
    let e = emulator();
    PackIndexStore::new(dynstore(&e), "")
        .add(NODE, 1)
        .expect("indexed");
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    run_as(&mut c, 100, &refs, true);
    run(&mut c, 101, &refs);
    assert!(!index_state(&e).index.contains(NODE));
}

/// A live writer with nothing to judge is asked about with its ledger alone
/// while the ledger has not moved: the sidecar is not read again.
#[test]
fn a_live_node_with_nothing_to_judge_costs_one_get_while_its_ledger_stands() {
    let e = emulator();
    writer_with_ledger(&e, 1, 8);
    let refs = Nothing::default();
    let mut c = PackCollector::new(dynstore(&e), "", 100);
    let before = gets(&e);
    run_as(&mut c, 100, &refs, true);
    assert_eq!(gets(&e) - before, 16 + 2, "index, ledger and sidecar");
    let before = gets(&e);
    for w in 101..104 {
        run_as(&mut c, w, &refs, true);
    }
    assert_eq!(gets(&e) - before, 3 * (16 + 1), "index and ledger only");
    ledger(&e).reserve(1, 16).expect("moved");
    let before = gets(&e);
    run_as(&mut c, 104, &refs, true);
    assert_eq!(gets(&e) - before, 16 + 2, "a moved ledger is read in full");
}
