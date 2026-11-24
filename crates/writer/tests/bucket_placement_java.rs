//! docs/specs/bucket-java-fixtures/SPEC.md: the Rust placement and stripe
//! agree with what the Java shell computed (`fixtures/bucket-manifests/placement.tsv`).

// Test fixtures are small files read whole.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::NodeLease;
use oss_writer::bucket::{BucketAssigner, BucketPlacement};

const NOW: i64 = 1_000_000;

fn lines() -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bucket-manifests/placement.tsv");
    std::fs::read_to_string(path)
        .expect("fixture")
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

fn lease(id: &str, role: &str, expires: i64, held: i32) -> NodeLease {
    let mut lease = NodeLease::new(id, &format!("e-{id}"), "10.0.0.1:9300", expires);
    lease.set_roles([role.to_owned()]);
    if held >= 0 {
        lease.held = held;
        lease.cap = 100;
    }
    lease
}

fn fleet() -> Vec<NodeLease> {
    let live = NOW + 60_000;
    let mut revoked = lease("node-6", "ingest", live, 10);
    revoked.revoked = true;
    vec![
        lease("node-0", "ingest", live, 10),
        lease("node-1", "ingest", live, 12),
        lease("node-2", "ingest", live, 50),
        lease("node-3", "search", live, 11),
        lease("node-4", "ingest", live, -1),
        lease("node-5", "ingest", live, 9),
        revoked,
        lease("node-7", "ingest", NOW - 1, 10),
        lease("node-10", "ingest", live, 10),
        lease("node-9", "ingest", live, 12),
    ]
}

fn is_writer(lease: &NodeLease) -> bool {
    lease.roles().iter().any(|r| r == "ingest")
}

fn ids(list: &str) -> Vec<String> {
    list.split(',').map(str::to_owned).collect()
}

#[test]
fn the_stripe_and_bucket_id_are_the_javas() {
    let mut checked = 0;
    for line in lines() {
        match line[0].as_str() {
            "stripe" => {
                assert_eq!(
                    BucketAssigner::stripe(&line[1]).to_string(),
                    line[2],
                    "{}",
                    line[1]
                );
                checked += 1;
            }
            "bucketid" => {
                let stripe: usize = line[1].parse().unwrap();
                let generation: u64 = line[2].parse().unwrap();
                assert_eq!(BucketAssigner::bucket_id(stripe, generation), line[3]);
                checked += 1;
            }
            _ => {}
        }
    }
    assert!(checked >= 19, "{checked}");
}

#[test]
fn the_ranking_and_the_keep_decision_are_the_javas() {
    let placement = BucketPlacement::new(0);
    let leases = fleet();
    let (mut ranks, mut keeps) = (0, 0);
    for line in lines() {
        match line[0].as_str() {
            "rank" => {
                let ranked = placement.rank(&line[1], &leases, NOW, is_writer);
                assert_eq!(ranked, ids(&line[2]), "{}", line[1]);
                ranks += 1;
            }
            "keeps" => {
                let kept = placement.keeps(&line[1], &line[2], &leases, NOW, is_writer);
                assert_eq!(kept.to_string(), line[3], "{} {}", line[1], line[2]);
                keeps += 1;
            }
            _ => {}
        }
    }
    assert_eq!((ranks, keeps), (64, 192));
}

#[test]
fn a_late_joiner_is_held_down_as_long_as_the_java_holds_it() {
    let placement = BucketPlacement::new(120_000);
    let early = fleet()[..3].to_vec();
    placement.observe(&early, 0);
    let mut checked = 0;
    for line in lines().iter().filter(|l| l[0] == "holddown") {
        let at: i64 = line[1].parse().unwrap();
        let mut view = early.clone();
        view.push(lease("node-5", "ingest", at + 60_000, 9));
        let ranked = placement.rank(&line[2], &view, at, is_writer);
        assert_eq!(ranked, ids(&line[3]), "{at} {}", line[2]);
        checked += 1;
    }
    assert_eq!(checked, 12);
}
