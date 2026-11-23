//! Refreshes, deltas, the digest and pruning (AC-6, AC-7, AC-8).

use oss_blob::{BlobStore, S3Op};
use oss_cluster::Membership;
use oss_format::NodeLogEpoch;

use crate::{BASE, Fleet, TTL, ids, lease};

/// Three nodes, each renewed once.
fn three(fleet: &Fleet) -> Vec<Membership> {
    ["n1", "n2", "n3"]
        .iter()
        .map(|id| {
            let node = fleet.node();
            node.renew(&lease(id)).unwrap();
            node
        })
        .collect()
}

#[test]
fn a_refresh_sees_every_live_member_and_delivers_joins_and_departures() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    let first = nodes[1].refresh().unwrap();
    assert_eq!(ids(&first.live), ["n1", "n2", "n3"]);
    assert_eq!(ids(&first.delta.joined), ["n1", "n2", "n3"]);
    assert!(first.delta.left.is_empty());
    // A renewal alone is neither a join nor a departure.
    fleet.clock.advance(TTL / 3);
    for (node, id) in nodes.iter().zip(["n1", "n2", "n3"]) {
        node.renew(&lease(id)).unwrap();
    }
    fleet.clock.advance(TTL);
    for (node, id) in nodes.iter().zip(["n1", "n2", "n3"]) {
        node.renew(&lease(id)).unwrap();
    }
    let renewed = nodes[1].refresh().unwrap();
    assert!(renewed.delta.is_empty(), "{:?}", renewed.delta);
    nodes[2].release("n3").unwrap();
    let left = nodes[1].refresh().unwrap();
    assert_eq!(ids(&left.live), ["n1", "n2"]);
    assert_eq!(ids(&left.delta.left), ["n3"]);
    assert!(left.delta.joined.is_empty());
    let joiner = fleet.node();
    joiner.renew(&lease("n4")).unwrap();
    let joined = nodes[1].refresh().unwrap();
    assert_eq!(ids(&joined.delta.joined), ["n4"]);
    assert_eq!(ids(&nodes[1].current()), ["n1", "n2", "n4"]);
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn a_node_sees_itself_before_any_refresh_does() {
    let fleet = Fleet::new();
    let node = fleet.node();
    assert!(node.current().is_empty());
    node.renew(&lease("n1")).unwrap();
    assert_eq!(ids(&node.current()), ["n1"]);
    node.refresh().unwrap();
    assert_eq!(ids(&node.current()), ["n1"], "not twice");
    fleet.clock.advance(TTL);
    assert_eq!(ids(&node.current()), ["n1"], "the refreshed copy");
    node.refresh().unwrap();
    assert!(node.current().is_empty(), "expired, and not added");
}

#[test]
fn a_refresh_of_unexpired_copies_reads_only_the_index() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    // n3 is not the digest writer (n1 is, once it has seen the others).
    nodes[2].refresh().unwrap();
    let before = fleet.total();
    nodes[2].refresh().unwrap();
    assert_eq!(fleet.total() - before, 1, "the index, nothing else");
}

#[test]
fn the_lowest_live_id_writes_the_digest_and_the_rest_read_it() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    for node in &nodes {
        node.refresh().unwrap();
    }
    let digest = || fleet.read("cluster/members/members-digest");
    let written = digest().expect("a digest");
    // n1 has seen n2 and n3 and writes; n2 saw n1, so it does not again.
    let (generation, _) = written;
    nodes[1].refresh().unwrap();
    nodes[2].refresh().unwrap();
    assert_eq!(digest().unwrap().0, generation, "only n1 writes it");
    let digest_value = oss_format::decode_digest(&digest().unwrap().1).unwrap();
    assert_eq!(ids(&digest_value.leases), ["n1", "n2", "n3"]);
    // Within a third of a lifetime n1 does not write it again.
    nodes[0].refresh().unwrap();
    assert_eq!(digest().unwrap().0, generation);
    // Everyone renews; the copies n3 holds run out; n1 writes a new digest
    // and n3 takes the leases from it with one read instead of three.
    fleet.clock.advance(TTL / 3);
    for (node, id) in nodes.iter().zip(["n1", "n2", "n3"]) {
        node.renew(&lease(id)).unwrap();
    }
    nodes[0].refresh().unwrap();
    assert_ne!(digest().unwrap().0, generation, "written again");
    fleet.clock.advance(TTL);
    for (node, id) in nodes.iter().zip(["n1", "n2", "n3"]) {
        node.renew(&lease(id)).unwrap();
    }
    nodes[0].refresh().unwrap();
    let before = fleet.count(S3Op::Get);
    let seen = nodes[2].refresh().unwrap();
    assert_eq!(ids(&seen.live), ["n1", "n2", "n3"]);
    assert_eq!(
        fleet.count(S3Op::Get) - before,
        2,
        "the index and the digest, no lease"
    );
}

#[test]
fn an_unreadable_digest_falls_back_to_the_leases() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    fleet
        .store
        .create_register(&format!("{BASE}cluster/members/members-digest"), b"junk")
        .unwrap();
    let before = fleet.count(S3Op::Get);
    let seen = nodes[2].refresh().unwrap();
    assert_eq!(ids(&seen.live), ["n1", "n2", "n3"]);
    assert_eq!(
        fleet.count(S3Op::Get) - before,
        5,
        "index, digest, three leases"
    );
}

#[test]
fn a_probe_sees_a_join_within_a_young_view() {
    let fleet = Fleet::new();
    let node = fleet.node();
    node.renew(&lease("n1")).unwrap();
    assert!(
        node.refresh_if_older_than(TTL / 2).unwrap().is_some(),
        "never refreshed"
    );
    let before = fleet.total();
    assert!(node.refresh_if_older_than(TTL / 2).unwrap().is_none());
    assert_eq!(fleet.total() - before, 1, "one probe of the index");
    fleet.node().renew(&lease("n2")).unwrap();
    let seen = node
        .refresh_if_older_than(TTL / 2)
        .unwrap()
        .expect("refreshed");
    assert_eq!(ids(&seen.delta.joined), ["n2"]);
    fleet.clock.advance(TTL / 2);
    assert!(
        node.refresh_if_older_than(TTL / 2).unwrap().is_some(),
        "aged out"
    );
}

#[test]
fn a_probe_is_made_at_most_once_an_interval() {
    let fleet = Fleet::new();
    let node = fleet
        .node()
        .with_probe_interval(std::time::Duration::from_secs(3600));
    node.renew(&lease("n1")).unwrap();
    node.refresh_if_older_than(TTL).unwrap();
    assert!(node.refresh_if_older_than(TTL).unwrap().is_none());
    let before = fleet.total();
    fleet.node().renew(&lease("n2")).unwrap();
    let after_join = fleet.total();
    assert!(node.refresh_if_older_than(TTL).unwrap().is_none());
    assert_eq!(fleet.total(), after_join, "answered from the view");
    assert!(after_join > before);
}

#[test]
fn a_lease_dead_for_ten_lifetimes_is_unlisted_and_deleted_where_verified() {
    for deletes in [false, true] {
        let fleet = Fleet::new();
        let watcher = fleet.node_with(fleet.config(deletes));
        watcher.renew(&lease("n0")).unwrap();
        fleet.node().renew(&lease("n1")).unwrap();
        let mut logged = lease("n2");
        logged.node_log = vec![NodeLogEpoch {
            epoch: 7,
            trim: 0,
            landed: 0,
        }];
        fleet.node().renew(&logged).unwrap();
        // Just under ten lifetimes past expiry: kept.
        fleet.clock.advance(TTL + 10 * TTL - 1);
        watcher.renew(&lease("n0")).unwrap();
        watcher.refresh().unwrap();
        assert_eq!(fleet.index(), ["n0", "n1", "n2"]);
        fleet.clock.advance(1);
        watcher.renew(&lease("n0")).unwrap();
        watcher.refresh().unwrap();
        assert_eq!(fleet.index(), ["n0", "n2"], "the epoch-holder is kept");
        assert_eq!(fleet.lease("n1").is_some(), !deletes);
        assert!(fleet.lease("n2").is_some());
        // Its epochs forgotten, it is pruned like any other.
        assert!(
            !watcher.forget_node_log("n2", &[]).unwrap(),
            "not the epochs checked"
        );
        assert!(watcher.forget_node_log("n2", &logged.node_log).unwrap());
        assert!(watcher.forget_node_log("n2", &[]).unwrap(), "none left");
        watcher.renew(&lease("n0")).unwrap();
        watcher.refresh().unwrap();
        assert_eq!(fleet.index(), ["n0"]);
    }
}

#[test]
fn a_live_node_keeps_its_write_log_epochs() {
    let fleet = Fleet::new();
    let mut logged = lease("n2");
    let epochs = vec![NodeLogEpoch {
        epoch: 7,
        trim: 0,
        landed: 0,
    }];
    logged.node_log = epochs.clone();
    fleet.node().renew(&logged).unwrap();
    let other = fleet.node();
    assert!(!other.forget_node_log("n2", &epochs).unwrap(), "live");
    assert!(other.forget_node_log("absent", &epochs).unwrap());
    assert_eq!(fleet.lease("n2").unwrap().node_log, epochs);
}

#[test]
fn a_listed_lease_that_is_gone_is_pruned_and_a_renewal_re_enrols() {
    let fleet = Fleet::new();
    let watcher = fleet.node_with(fleet.config(true));
    watcher.renew(&lease("n0")).unwrap();
    let node = fleet.node();
    node.renew(&lease("n1")).unwrap();
    // The lease deleted while the index still names it, as after a release
    // whose index swap was lost.
    let key = format!("{BASE}cluster/members/lease-n1");
    let generation = fleet.read("cluster/members/lease-n1").unwrap().0;
    assert!(fleet.store.delete_register(&key, generation).unwrap());
    watcher.refresh().unwrap();
    assert_eq!(fleet.index(), ["n0"]);
    assert!(fleet.lease("n1").is_none(), "the marker deleted too");
    // n1 comes back: its register moved underneath, so it enrols again.
    node.renew(&lease("n1")).unwrap();
    assert!(node.is_enrolled());
    assert_eq!(fleet.index(), ["n0", "n1"]);
}

#[test]
fn a_missing_index_lists_nothing_and_a_refresh_sees_no_one() {
    let fleet = Fleet::new();
    let node = fleet.node();
    let seen = node.refresh().unwrap();
    assert!(seen.live.is_empty());
    assert_eq!(fleet.total(), 1, "one read, no listing");
    assert!(node.enrolled_ids().unwrap().is_empty());
    node.renew(&lease("n1")).unwrap();
    assert_eq!(node.enrolled_ids().unwrap(), ["n1"]);
    assert_eq!(node.read("n1").unwrap().unwrap().node_id, "n1");
    assert!(node.read("absent").unwrap().is_none());
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn an_unreadable_lease_is_skipped() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    let key = format!("{BASE}cluster/members/lease-n2");
    let generation = fleet.read("cluster/members/lease-n2").unwrap().0;
    fleet.store.swap_register(&key, generation, b"[]").unwrap();
    fleet.clock.advance(TTL);
    nodes[0].renew(&lease("n1")).unwrap();
    nodes[2].renew(&lease("n3")).unwrap();
    let seen = nodes[0].refresh().unwrap();
    assert_eq!(ids(&seen.live), ["n1", "n3"]);
    assert!(nodes[0].read("n2").is_err());
}

#[test]
fn a_delta_is_empty_only_with_nobody_joined_or_left() {
    let mut delta = oss_cluster::MembershipDelta::default();
    assert!(delta.is_empty());
    delta.left.push(lease("n1"));
    assert!(!delta.is_empty());
    delta.left.clear();
    delta.joined.push(lease("n1"));
    assert!(!delta.is_empty());
}

#[test]
fn a_restart_under_the_same_id_is_a_departure_and_a_join() {
    let fleet = Fleet::new();
    let watcher = fleet.node();
    watcher.renew(&lease("n0")).unwrap();
    fleet.node().renew(&lease("n1")).unwrap();
    watcher.refresh().unwrap();
    // The same id and address, a new process: a fast restart.
    let mut restarted = lease("n1");
    restarted.ephemeral_id = "en1-again".to_owned();
    fleet.node().renew(&restarted).unwrap();
    fleet.clock.advance(TTL);
    watcher.renew(&lease("n0")).unwrap();
    fleet.node().renew(&restarted).unwrap();
    let seen = watcher.refresh().unwrap();
    assert_eq!(ids(&seen.delta.left), ["n1"]);
    assert_eq!(ids(&seen.delta.joined), ["n1"]);
    assert_eq!(seen.delta.joined[0].ephemeral_id, "en1-again");
}

/// Gets of a reader's refresh made `gap` after it last read the digest,
/// once one copy it took from that digest has run out.
fn gets_after_digest_read(gap: i64) -> u64 {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    // The reader reads (or, seeing nobody lower yet, writes) the digest
    // just before the copy of n2 in it runs out.
    fleet.clock.advance(TTL - gap);
    nodes[0].renew(&lease("n1")).unwrap();
    nodes[2].renew(&lease("n3")).unwrap();
    nodes[2].refresh().unwrap();
    fleet.clock.advance(gap);
    nodes[1].renew(&lease("n2")).unwrap();
    let before = fleet.count(S3Op::Get);
    let seen = nodes[2].refresh().unwrap();
    assert_eq!(ids(&seen.live), ["n1", "n2", "n3"]);
    fleet.count(S3Op::Get) - before
}

#[test]
fn a_reader_takes_the_digest_at_most_once_a_tenth_of_a_lifetime() {
    assert_eq!(
        gets_after_digest_read(TTL / 10 - 1),
        2,
        "the index and the lease run out, no digest"
    );
    assert_eq!(
        gets_after_digest_read(TTL / 10),
        3,
        "the index, the digest and the lease it still lacks"
    );
}

#[test]
fn a_copy_run_out_in_the_digest_is_not_taken_from_it() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    nodes[2].refresh().unwrap();
    nodes[0].refresh().unwrap();
    // n2 stops renewing; n1 writes a digest naming it a millisecond before
    // its lease runs out.
    fleet.clock.advance(TTL - 1);
    nodes[0].renew(&lease("n1")).unwrap();
    nodes[2].renew(&lease("n3")).unwrap();
    nodes[0].refresh().unwrap();
    fleet.clock.advance(1);
    assert_eq!(ids(&nodes[2].refresh().unwrap().live), ["n1", "n3"]);
    // A tenth of a lifetime on, the reader's copies are fresh but for the
    // dead n2's, which the digest has no newer word on.
    fleet.clock.advance(TTL / 10);
    nodes[0].renew(&lease("n1")).unwrap();
    nodes[2].renew(&lease("n3")).unwrap();
    let before = fleet.count(S3Op::Get);
    nodes[2].refresh().unwrap();
    assert_eq!(
        fleet.count(S3Op::Get) - before,
        2,
        "the index and n2's lease; the digest is not due"
    );
}

#[test]
fn a_lower_member_gone_a_lifetime_past_its_lease_no_longer_holds_the_digest() {
    for (late, writes) in [(TTL - 1, false), (TTL, true)] {
        let fleet = Fleet::new();
        let low = fleet.node();
        low.renew(&lease("n1")).unwrap();
        let node = fleet.node();
        node.renew(&lease("n2")).unwrap();
        low.refresh().unwrap();
        node.refresh().unwrap();
        let digest = || fleet.read("cluster/members/members-digest").map(|(g, _)| g);
        let before = digest();
        // n1 stops renewing; n2 refreshes next `late` after n1's lease ran out.
        fleet.clock.advance(TTL + late);
        node.renew(&lease("n2")).unwrap();
        node.refresh().unwrap();
        assert_eq!(digest() != before, writes, "{late}");
    }
}

fn logged(id: &str) -> oss_format::NodeLease {
    let mut l = lease(id);
    l.node_log = vec![NodeLogEpoch {
        epoch: 7,
        trim: 0,
        landed: 0,
    }];
    l
}

#[test]
fn a_dead_member_naming_epochs_is_reported_on_every_refresh_with_no_extra_read() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    nodes[2].renew(&logged("n3")).unwrap();
    nodes[1].refresh().unwrap();
    fleet.clock.advance(TTL + 1);
    for (node, id) in nodes.iter().zip(["n1", "n2"]) {
        node.renew(&lease(id)).unwrap();
    }
    let first = nodes[1].refresh().unwrap();
    assert_eq!(ids(&first.dead_with_epochs), ["n3"]);
    assert_eq!(first.dead_with_epochs[0].node_log.len(), 1);
    // A node that never saw it live sees it too, and it is not a delta.
    let late = fleet.node();
    late.renew(&lease("n4")).unwrap();
    let seen = late.refresh().unwrap();
    assert_eq!(ids(&seen.dead_with_epochs), ["n3"]);
    // Reported again at the next refresh, from the read the refresh already makes.
    let before = fleet.count(S3Op::Get);
    let again = nodes[1].refresh().unwrap();
    assert_eq!(ids(&again.dead_with_epochs), ["n3"]);
    let reads = fleet.count(S3Op::Get) - before;
    assert!(reads <= 3, "{reads}");
}

#[test]
fn a_dead_member_naming_no_epochs_or_a_live_one_is_not_reported() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    nodes[2].renew(&logged("n3")).unwrap();
    fleet.clock.advance(TTL + 1);
    nodes[0].renew(&lease("n1")).unwrap();
    // n2 expired with no epochs, n3 expired naming one.
    let seen = nodes[0].refresh().unwrap();
    assert_eq!(ids(&seen.dead_with_epochs), ["n3"]);
    assert_eq!(ids(&seen.live), ["n1"]);
}

#[test]
fn a_revoked_member_naming_epochs_is_reported_as_revoked() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    nodes[2].renew(&logged("n3")).unwrap();
    fleet.clock.advance(TTL + 1);
    nodes[0].renew(&lease("n1")).unwrap();
    assert!(nodes[0].revoke("n3", None));
    let seen = nodes[0].refresh().unwrap();
    assert_eq!(ids(&seen.dead_with_epochs), ["n3"]);
    assert!(seen.dead_with_epochs[0].revoked);
    assert!(!seen.live.iter().any(|l| l.node_id == "n3"));
}

// docs/specs/proactive-takeover AC-11
#[test]
fn a_dead_lease_forgotten_after_its_log_is_collected_stops_costing_a_read_each_refresh() {
    let fleet = Fleet::new();
    let nodes = three(&fleet);
    nodes[2].renew(&logged("n3")).unwrap();
    fleet.clock.advance(TTL + 1);
    for (node, id) in nodes.iter().zip(["n1", "n2"]) {
        node.renew(&lease(id)).unwrap();
    }
    let reads = |node: &Membership| {
        let before = fleet.count(S3Op::Get);
        let seen = node.refresh().unwrap();
        (fleet.count(S3Op::Get) - before, seen)
    };
    let (naming, seen) = reads(&nodes[1]);
    assert_eq!(ids(&seen.dead_with_epochs), ["n3"]);
    assert!(
        nodes[1]
            .forget_node_log("n3", &seen.dead_with_epochs[0].node_log)
            .unwrap()
    );
    // Long dead, with the live nodes renewed: the lease is pruned.
    for _ in 0..4 {
        fleet.clock.advance(1_000 * TTL);
        for (node, id) in nodes.iter().zip(["n1", "n2"]) {
            node.renew(&lease(id)).unwrap();
        }
        nodes[1].refresh().unwrap();
    }
    let (forgotten, seen) = reads(&nodes[1]);
    assert!(seen.dead_with_epochs.is_empty());
    assert!(
        forgotten < naming,
        "{forgotten} reads after the forget, {naming} before"
    );
}
