//! Which logged commit a successor may open from (docs/specs/open-from-
//! logged-commit): decided by the manifests, never by log positions.

// ARITH: test sizes are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::time::Duration;

use oss_format::encode_manifest;
use oss_writer::log_commit::CommitRecord;
use oss_writer::pack::SWAP_DEADLINE_MILLIS;
use oss_writer::takeover::logged::{OPEN_MARGIN_MILLIS, select};

const NOW: u64 = 10_000_000;
/// The longest age a candidate may have.
const USABLE: u64 = SWAP_DEADLINE_MILLIS - OPEN_MARGIN_MILLIS;

fn manifest(term: u64, stage_seq: i64) -> Vec<u8> {
    encode_manifest(term, Some("node-1"), &[], 5, stage_seq).expect("encodes")
}

fn record(term: u64, stage_seq: i64, age: u64) -> CommitRecord {
    CommitRecord {
        oldest_pack_millis: i64::try_from(NOW - age).expect("fits"),
        covered: None,
        manifest: String::from_utf8(manifest(term, stage_seq)).expect("utf8"),
    }
}

fn register(term: u64, stage_seq: i64) -> Option<(Vec<u8>, u64)> {
    Some((manifest(term, stage_seq), 9))
}

fn pick(reg: &Option<(Vec<u8>, u64)>, won: i64, records: &[CommitRecord]) -> Option<(u64, i64)> {
    let r = reg.as_ref().map(|(b, g)| (b.as_slice(), *g));
    select(r, won, records, NOW).map(|c| (c.term, c.stage_seq))
}

// AC-1
#[test]
fn a_newer_commit_is_chosen_and_carries_the_register_generation() {
    let reg = register(3, 4);
    let c = select(
        reg.as_ref().map(|(b, g)| (b.as_slice(), *g)),
        5,
        &[record(3, 6, 1_000)],
        NOW,
    )
    .expect("chosen");
    assert_eq!((c.term, c.stage_seq, c.generation), (3, 6, 9));
    assert_eq!(
        c.packed_ago,
        Duration::from_millis(1_000 + OPEN_MARGIN_MILLIS)
    );
}

// AC-3
#[test]
fn a_commit_not_newer_than_the_register_is_ignored() {
    let reg = register(3, 4);
    assert_eq!(pick(&reg, 5, &[record(3, 4, 0)]), None, "equal");
    assert_eq!(pick(&reg, 5, &[record(3, 2, 0)]), None, "older, same term");
    assert_eq!(pick(&reg, 5, &[record(2, 9, 0)]), None, "an older term");
    assert_eq!(pick(&register(4, 0), 5, &[record(3, 9, 0)]), None);
}

// AC-3
#[test]
fn the_registers_without_a_stage_sequence_or_absent_refuse_every_candidate() {
    assert_eq!(pick(&register(3, -1), 5, &[record(3, 6, 0)]), None);
    assert_eq!(pick(&None, 5, &[record(3, 6, 0)]), None);
}

// AC-3
#[test]
fn a_candidate_must_come_from_a_term_before_the_heads() {
    let reg = register(3, 4);
    assert_eq!(
        pick(&reg, 5, &[record(5, 9, 0)]),
        None,
        "equal to the head's"
    );
    assert_eq!(pick(&reg, 5, &[record(6, 9, 0)]), None, "above it");
    assert_eq!(pick(&reg, 5, &[record(4, 0, 0)]), Some((4, 0)));
}

// AC-3
#[test]
fn a_pack_age_is_judged_against_the_deadline_less_the_margin() {
    let reg = register(3, 4);
    assert_eq!(pick(&reg, 5, &[record(3, 6, USABLE - 1)]), Some((3, 6)));
    assert_eq!(pick(&reg, 5, &[record(3, 6, USABLE)]), None);
    assert_eq!(pick(&reg, 5, &[record(3, 6, SWAP_DEADLINE_MILLIS)]), None);
}

// AC-3
#[test]
fn a_record_with_no_age_or_no_manifest_is_ignored() {
    let reg = register(3, 4);
    let mut unaged = record(3, 6, 0);
    unaged.oldest_pack_millis = 0;
    assert_eq!(pick(&reg, 5, &[unaged]), None);
    let mut broken = record(3, 6, 0);
    broken.manifest = "not a manifest".to_owned();
    assert_eq!(pick(&reg, 5, &[broken]), None);
}

// AC-3
#[test]
fn across_writers_the_greatest_term_and_sequence_wins() {
    let reg = register(2, 1);
    let records = [record(3, 2, 0), record(4, 0, 0), record(3, 8, 0)];
    assert_eq!(pick(&reg, 6, &records), Some((4, 0)));
    let same = [record(3, 2, 0), record(3, 8, 0), record(3, 5, 0)];
    assert_eq!(pick(&reg, 6, &same), Some((3, 8)));
}
