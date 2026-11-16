//! What membership costs (AC-9), held to docs/request-budgets.md: each
//! operation on its own, and an idle hour of three nodes renewing and
//! refreshing every ten seconds on a simulated clock.

use oss_blob::S3Op;
use oss_testkit::budget;

use crate::{Fleet, TTL, lease};

/// Checks the requests `path` makes against the row `key`.
fn gate<T>(fleet: &Fleet, key: &str, path: impl FnOnce() -> T) -> T {
    let before = fleet.total();
    let out = path();
    budget::check(key, fleet.total() - before, key);
    out
}

#[test]
fn lease_operations_cost_what_the_table_says() {
    let fleet = Fleet::new();
    let n1 = fleet.node();
    let n2 = fleet.node();
    gate(&fleet, "lease.first_renewal.requests", || {
        n1.renew(&lease("n1")).unwrap()
    });
    n2.renew(&lease("n2")).unwrap();
    gate(&fleet, "lease.renewal.requests", || {
        n1.renew(&lease("n1")).unwrap()
    });
    // n2 reads both leases once, and n1 writes the digest.
    n1.refresh().unwrap();
    n2.refresh().unwrap();
    gate(&fleet, "lease.refresh.requests", || n2.refresh().unwrap());
    fleet.clock.advance(TTL / 3);
    n1.renew(&lease("n1")).unwrap();
    n2.renew(&lease("n2")).unwrap();
    n1.refresh().unwrap();
    fleet.clock.advance(TTL);
    n1.renew(&lease("n1")).unwrap();
    n2.renew(&lease("n2")).unwrap();
    n1.refresh().unwrap();
    let seen = gate(&fleet, "lease.refresh_digest.requests", || {
        n2.refresh().unwrap()
    });
    assert_eq!(seen.live.len(), 2);
    gate(&fleet, "lease.release.requests", || {
        n2.release("n2").unwrap()
    });
    fleet.clock.advance(TTL);
    n2.renew(&lease("n2")).unwrap();
    fleet.clock.advance(TTL);
    assert!(gate(&fleet, "lease.revoke.requests", || n1
        .revoke("n2", Some("en2"))));
}

/// Requests one node made in the hour.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Cost {
    gets: u64,
    swaps: u64,
    lists: u64,
}

#[test]
fn an_idle_hour_costs_what_the_table_says() {
    let fleet = Fleet::new();
    let ids = ["n1", "n2", "n3"];
    let nodes: Vec<_> = ids.iter().map(|_| fleet.node()).collect();
    for (node, id) in nodes.iter().zip(ids) {
        node.renew(&lease(id)).unwrap();
    }
    for node in &nodes {
        node.refresh().unwrap();
    }
    let mut costs = [Cost::default(); 3];
    for _ in 0..360 {
        fleet.clock.advance(10_000);
        for (i, (node, id)) in nodes.iter().zip(ids).enumerate() {
            let (gets, puts, lists) = (
                fleet.count(S3Op::Get),
                fleet.count(S3Op::Put),
                fleet.count(S3Op::List),
            );
            node.renew(&lease(id)).unwrap();
            node.refresh().unwrap();
            costs[i].gets += fleet.count(S3Op::Get) - gets;
            costs[i].swaps += fleet.count(S3Op::Put) - puts;
            costs[i].lists += fleet.count(S3Op::List) - lists;
        }
    }
    let [writer, second, third] = costs;
    let detail = format!("writer {writer:?}, others {second:?} {third:?}");
    budget::check("lease.idle.writer_swaps_per_hour", writer.swaps, &detail);
    budget::check("lease.idle.writer_gets_per_hour", writer.gets, &detail);
    budget::check("lease.idle.reader_swaps_per_hour", second.swaps, &detail);
    budget::check("lease.idle.reader_gets_per_hour", second.gets, &detail);
    assert_eq!(second, third, "{detail}");
    let lists = costs.iter().map(|c| c.lists).sum();
    budget::check("lease.idle.lists_per_hour", lists, &detail);
}
