//! The Java's `ZoneCostMeterTests` fleet, modelled over placement (AC-4):
//! three zones, two search nodes in each that coordinate and read, six
//! shards, and twenty rounds of a search from every node over every shard.
//! A shard read on the coordinator itself moves no bytes; any other is a
//! forward and its answer, counted on the coordinator's meter.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use oss_cluster::placement::{HotShardSpread, ShardKey, SpreadConfig, readers_for};
use oss_cluster::zones::{Path, ZonePreference, ZoneTraffic, zone_of};
use oss_format::NodeLease;

const SHARDS: i32 = 6;
const ROUNDS: usize = 20;
const ZONES: [&str; 3] = ["zone-a", "zone-b", "zone-c"];
const SEARCH: &str = "indices:data/read/search[forwarded]";
/// A forwarded shard search and its answer, payload bytes.
const REQUEST_BYTES: u64 = 300;
const ANSWER_BYTES: u64 = 4_000;
/// Time between rounds: twenty searches a second of each shard from each
/// coordinator, well above the default one a second that counts as hot.
const ROUND_NANOS: i64 = 50_000_000;

fn fleet(zoned: bool) -> Vec<NodeLease> {
    (0..2 * ZONES.len())
        .map(|i| {
            let id = format!("s{i}");
            let mut lease = NodeLease::new(&id, &format!("e{id}"), &format!("a{id}"), i64::MAX);
            lease.set_roles(["search".to_owned()]);
            lease.set_zone(zoned.then(|| ZONES[i % ZONES.len()].to_owned()));
            lease
        })
        .collect()
}

/// Runs the searches; returns the coordinators' meters added up, by path.
fn run(members: &[NodeLease], preference: ZonePreference) -> [u64; 3] {
    let clock = Arc::new(AtomicI64::new(1_000_000_000_000));
    let coordinators: Vec<_> = members
        .iter()
        .map(|lease| {
            let nanos = Arc::clone(&clock);
            let spread = HotShardSpread::new(
                SpreadConfig::default(),
                Arc::new(move || nanos.load(Ordering::Relaxed)),
            )
            .unwrap();
            (
                lease.node_id.as_str(),
                lease.zone(),
                spread,
                ZoneTraffic::new(),
            )
        })
        .collect();
    for _ in 0..ROUNDS {
        clock.fetch_add(ROUND_NANOS, Ordering::Relaxed);
        for (id, zone, spread, meter) in &coordinators {
            for shard in 0..SHARDS {
                let key = ShardKey::new("meter", shard);
                let ranked = readers_for(key, members.iter(), "search", spread.max_readers());
                let ordered = spread.order(key, &ranked);
                let chosen = preference.order(key, &ordered, members, "search", *zone, spread);
                let reader = chosen[0];
                if reader != *id {
                    let path = Path::to_peer(*zone, members, reader);
                    meter.record(path, SEARCH, REQUEST_BYTES);
                    meter.record(path, SEARCH, ANSWER_BYTES);
                }
            }
        }
    }
    Path::ALL.map(|path| coordinators.iter().map(|(.., m)| m.total(path)).sum())
}

#[test]
fn the_zone_preference_keeps_search_bytes_inside_the_zone() {
    let members = fleet(true);
    // No shard is ever hot enough to prefer a reader in the coordinator's zone.
    let off = ZonePreference::new(f64::MAX, 8).unwrap();
    let [_, before, unknown] = run(&members, off);
    let [same, after, _] = run(&members, ZonePreference::default());
    assert_eq!(unknown, 0, "every node declares a zone");
    // Six coordinators, six shards, twenty rounds: 720 shard searches. With
    // the preference off about two in three go to another zone.
    assert!(
        before > 300 * (REQUEST_BYTES + ANSWER_BYTES),
        "before {before}"
    );
    assert!(
        after * 20 <= before,
        "cross-zone search bytes {before} -> {after}, more than 5% left"
    );
    assert!(
        same > 0,
        "the zone mate serves what the coordinator does not"
    );
    for lease in &members {
        assert!(zone_of(&members, &lease.node_id).is_some());
    }
}

#[test]
fn a_fleet_without_zones_reads_as_unmeasured() {
    let members = fleet(false);
    let [same, cross, unknown] = run(&members, ZonePreference::default());
    assert_eq!((same, cross), (0, 0));
    assert!(unknown > 0, "the forwards still count, as unknown");
}
