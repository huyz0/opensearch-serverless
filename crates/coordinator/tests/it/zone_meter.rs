//! docs/specs/coordinator-zone-meter: a coordinator counts the bytes it exchanges by zone.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::Cell;

use oss_cluster::zones::{Path, ZoneTraffic};
use oss_coordinator::fanout::run;
use oss_coordinator::zone_meter::ZoneMeter;
use oss_format::NodeLease;

fn lease(id: &str, zone: Option<&str>) -> NodeLease {
    let mut lease = NodeLease::new(id, "e", "127.0.0.1:9300", 1_000);
    lease.set_zone(zone.map(str::to_owned));
    lease
}

fn sized(answer: &[u8]) -> u64 {
    answer.len() as u64
}

#[test]
fn same_and_cross_zone_exchanges_count_under_their_paths() {
    let members = [
        lease("n1", Some("a")),
        lease("n2", Some("a")),
        lease("n3", Some("b")),
    ];
    let traffic = ZoneTraffic::new();
    let meter = ZoneMeter::new(&traffic, Some("a"), &members);
    meter
        .exchange(
            "n2",
            "search",
            10,
            || Ok::<_, ()>(vec![0u8; 30]),
            |a| sized(a),
        )
        .expect("ok");
    meter
        .exchange(
            "n3",
            "search",
            7,
            || Ok::<_, ()>(vec![0u8; 100]),
            |a| sized(a),
        )
        .expect("ok");
    assert_eq!(traffic.total(Path::SameZone), 40);
    assert_eq!(traffic.total(Path::CrossZone), 107);
    assert_eq!(traffic.by_action(Path::CrossZone).get("search"), Some(&107));
    assert_eq!(traffic.total(Path::Unknown), 0);
}

#[test]
fn a_missing_zone_or_lease_counts_as_unknown() {
    let members = [lease("n1", None), lease("n2", Some("a"))];
    let traffic = ZoneTraffic::new();
    // The peer declares no zone.
    ZoneMeter::new(&traffic, Some("a"), &members)
        .exchange("n1", "bulk", 1, || Ok::<_, ()>(vec![0u8; 2]), |a| sized(a))
        .expect("ok");
    // This node declares none.
    ZoneMeter::new(&traffic, None, &members)
        .exchange("n2", "bulk", 4, || Ok::<_, ()>(vec![0u8; 8]), |a| sized(a))
        .expect("ok");
    // The peer is not among the members.
    ZoneMeter::new(&traffic, Some("a"), &members)
        .exchange(
            "ghost",
            "bulk",
            16,
            || Ok::<_, ()>(vec![0u8; 32]),
            |a| sized(a),
        )
        .expect("ok");
    assert_eq!(traffic.total(Path::Unknown), 63);
    assert_eq!(
        traffic.total(Path::SameZone) + traffic.total(Path::CrossZone),
        0
    );
}

#[test]
fn a_failed_call_counts_its_request_only() {
    let members = [lease("n3", Some("b"))];
    let traffic = ZoneTraffic::new();
    let meter = ZoneMeter::new(&traffic, Some("a"), &members);
    let out = meter.exchange(
        "n3",
        "search",
        25,
        || Err::<Vec<u8>, _>("down"),
        |a| sized(a),
    );
    assert_eq!(out, Err("down"));
    assert_eq!(traffic.total(Path::CrossZone), 25);
}

#[test]
fn a_successful_call_returns_its_answer_once() {
    let members = [lease("n2", Some("a"))];
    let traffic = ZoneTraffic::new();
    let meter = ZoneMeter::new(&traffic, Some("a"), &members);
    let calls = Cell::new(0);
    let out = meter
        .exchange(
            "n2",
            "search",
            3,
            || {
                calls.set(calls.get() + 1);
                Ok::<_, ()>(vec![1u8, 2, 3])
            },
            |a| sized(a),
        )
        .expect("ok");
    assert_eq!(out, [1, 2, 3]);
    assert_eq!(calls.get(), 1);
}

#[test]
fn a_fan_out_counts_each_exchange_once() {
    let members = [
        lease("n0", Some("a")),
        lease("n1", Some("b")),
        lease("n2", Some("c")),
    ];
    let traffic = ZoneTraffic::new();
    let meter = ZoneMeter::new(&traffic, Some("a"), &members);
    let shards: Vec<(usize, usize)> = (0..8).map(|i| (i % 3, 10 + i)).collect();
    let nodes = ["n0", "n1", "n2"];
    let out = run(4, &shards, |(node, size)| {
        meter.exchange(
            nodes[*node],
            "search",
            5,
            || Ok::<_, ()>(vec![0u8; *size]),
            |a| sized(a),
        )
    });
    assert!(out.iter().all(Result::is_ok));
    let want = |zone: usize| -> u64 {
        shards
            .iter()
            .filter(|(node, _)| *node == zone)
            .map(|(_, size)| (5 + size) as u64)
            .sum()
    };
    assert_eq!(traffic.total(Path::SameZone), want(0));
    assert_eq!(traffic.total(Path::CrossZone), want(1) + want(2));
}
