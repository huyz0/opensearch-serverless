//! A view between two refreshes (AC-11).

use oss_cluster::Membership;

use crate::{Fleet, TTL, lease};

/// Three nodes, each renewed once.
fn nodes_of(fleet: &Fleet) -> Vec<Membership> {
    ["n1", "n2", "n3"]
        .iter()
        .map(|id| {
            let node = fleet.node();
            node.renew(&lease(id)).unwrap();
            node
        })
        .collect()
}

// AC-11
#[test]
fn a_view_between_two_refreshes_never_holds_an_expired_member() {
    let fleet = Fleet::new();
    let nodes = nodes_of(&fleet);
    for node in &nodes {
        node.refresh().unwrap();
    }
    // Three nodes renewing and refreshing every third of a lifetime plus
    // what a tick takes, as a booted node does: at no instant is any of
    // them lapsed, so no node's view of the others may say one is.
    let period = TTL / 3 + 100;
    for tick in 0..40 {
        fleet.clock.advance(period);
        for (node, id) in nodes.iter().zip(["n1", "n2", "n3"]) {
            node.renew(&lease(id)).unwrap();
            node.refresh().unwrap();
        }
        // Just before the next tick: the longest a view has gone unread.
        fleet.clock.advance(period - 1);
        let now = fleet.clock.now();
        for (i, node) in nodes.iter().enumerate() {
            let expired: Vec<_> = node
                .current()
                .iter()
                .filter(|m| m.is_expired_at(now))
                .map(|m| m.node_id.clone())
                .collect();
            assert!(
                expired.is_empty(),
                "tick {tick}: node {i} sees {expired:?} expired"
            );
        }
        fleet.clock.advance(-(period - 1));
    }
}
