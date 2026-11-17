//! The zone travels in the lease and the digest (zones AC-2).

use oss_cluster::zones::{Path, zone_of};

use crate::{Fleet, TTL, lease};

/// `id`'s lease, in `zone` when one is given.
fn zoned(id: &str, zone: Option<&str>) -> oss_format::NodeLease {
    let mut lease = lease(id);
    lease.set_zone(zone.map(str::to_owned));
    lease
}

#[test]
fn a_zone_survives_renewal_and_reaches_peers_through_the_digest() {
    let fleet = Fleet::new();
    let zones = [("n1", Some("az-a")), ("n2", Some("az-b")), ("n3", None)];
    let nodes: Vec<_> = zones
        .iter()
        .map(|(id, zone)| {
            let node = fleet.node();
            node.renew(&zoned(id, *zone)).unwrap();
            node
        })
        .collect();
    for node in &nodes {
        node.refresh().unwrap();
    }
    fleet.clock.advance(TTL / 3);
    for (node, (id, zone)) in nodes.iter().zip(zones) {
        node.renew(&zoned(id, zone)).unwrap();
    }
    assert_eq!(fleet.lease("n1").unwrap().zone(), Some("az-a"), "renewed");
    assert_eq!(fleet.lease("n3").unwrap().zone(), None);
    nodes[0].refresh().unwrap();
    let digest = fleet.read("cluster/members/members-digest").unwrap().1;
    let digest = oss_format::decode_digest(&digest).unwrap();
    assert_eq!(zone_of(&digest.leases, "n2"), Some("az-b"), "in the digest");
    assert_eq!(zone_of(&digest.leases, "n3"), None);
    let seen = nodes[2].refresh().unwrap().live;
    assert_eq!(zone_of(&seen, "n1"), Some("az-a"));
    assert_eq!(zone_of(&seen, "n2"), Some("az-b"));
    assert_eq!(zone_of(&seen, "n3"), None);
    let local = zone_of(&seen, "n1");
    assert_eq!(Path::to_peer(local, &seen, "n2"), Path::CrossZone);
    assert_eq!(Path::to_peer(local, &seen, "n1"), Path::SameZone);
    assert_eq!(Path::to_peer(local, &seen, "n3"), Path::Unknown);
    assert_eq!(Path::to_peer(local, &seen, "gone"), Path::Unknown);
    assert_eq!(Path::to_peer(None, &seen, "n1"), Path::Unknown);
}
