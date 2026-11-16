//! Fleet size, hand-offs, the writer for a shard and the takeover winner
//! against the Java's answers (AC-5).

use std::collections::HashMap;

use oss_cluster::placement::{FleetView, Holding, ShardKey};

use crate::{NOW, TTL, fleet, rows};

fn or_dash(answer: Option<&str>) -> &str {
    answer.unwrap_or("-")
}

#[test]
fn every_fleet_decision_is_the_javas() {
    let fleet = fleet();
    let departed: HashMap<&str, &str> = [("gone-a", "az-a")].into();
    let rows = rows("fleet-decisions.txt");
    assert_eq!(rows.len(), 1017);
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for row in &rows {
        let view = FleetView {
            members: &fleet,
            now: NOW,
            ttl: TTL,
            local: &row[1],
            holding: Holding {
                held: row[2].parse().unwrap(),
                cap: 100,
            },
        };
        *kinds.entry(row[0].clone()).or_default() += 1;
        match row[0].as_str() {
            "size" => {
                let s = view.fleet_size();
                assert_eq!(format!("{},{},{}", s.members, s.wanted, s.demand), row[3]);
            }
            "room" => assert_eq!(or_dash(view.member_with_room()), row[3], "{row:?}"),
            "mean" => assert_eq!(view.fleet_mean_held(), row[3].parse::<f64>().unwrap()),
            "below" => assert_eq!(or_dash(view.member_below_mean()), row[3], "{row:?}"),
            "handoff" => assert_eq!(or_dash(view.hand_off_target()), row[3], "{row:?}"),
            "writer" => {
                let key = ShardKey::new(&row[3], row[4].parse().unwrap());
                assert_eq!(or_dash(view.writer_for(key)), row[5], "{row:?}");
            }
            "takeover" => {
                let key = ShardKey::new(&row[4], row[5].parse().unwrap());
                let dead = row[6].as_str();
                let winner = view.takeover_winner(key, dead, row[3].parse().unwrap(), || {
                    departed.get(dead).map(|z| (*z).to_owned())
                });
                assert_eq!(or_dash(winner), row[7], "{row:?}");
            }
            other => panic!("unknown row {other}"),
        }
    }
    assert_eq!(kinds["takeover"], 864);
    assert_eq!(kinds["writer"], 108);
}

#[test]
fn zones_compare_as_the_javas() {
    use oss_cluster::placement::{ZonePreference, crosses, normalise, same, zone_of};
    assert_eq!(normalise(Some("  az-a ")), Some("az-a"));
    assert_eq!(normalise(Some("   ")), None);
    assert_eq!(normalise(None), None);
    assert!(same(Some("a"), Some("a")));
    assert!(!same(None, None), "no zone matches nothing");
    assert!(!same(Some("a"), Some("b")));
    assert!(crosses(Some("a"), Some("b")));
    assert!(
        !crosses(Some("a"), None) && !crosses(None, Some("b")) && !crosses(Some("a"), Some("a"))
    );
    let fleet = fleet();
    assert_eq!(zone_of(&fleet, "n02"), Some("az-b"));
    assert_eq!(zone_of(&fleet, "n04"), None, "no zone in its lease");
    assert_eq!(zone_of(&fleet, "absent"), None);
    assert!(ZonePreference::new(-0.5, 8).is_none());
    assert!(ZonePreference::new(f64::NAN, 8).is_none());
    assert_eq!(ZonePreference::new(1.0, 8), Some(ZonePreference::default()));
}

#[test]
fn an_assignment_needs_a_positive_term() {
    use oss_cluster::placement::ShardAssignment;
    assert!(ShardAssignment::new("logs", 0, 0).is_none());
    let a = ShardAssignment::new("logs", 3, 7).unwrap();
    assert_eq!((a.index(), a.shard(), a.term()), ("logs", 3, 7));
}

#[test]
fn a_departed_zone_is_read_once_and_a_failed_read_again() {
    use oss_cluster::placement::DepartedZones;
    use std::cell::Cell;
    let zones = DepartedZones::default();
    let reads = Cell::new(0);
    let lease = fleet().into_iter().find(|l| l.node_id == "n02").unwrap();
    let read = || {
        reads.set(reads.get() + 1);
        Ok::<_, ()>(Some(lease.clone()))
    };
    assert_eq!(zones.zone_of("n02", read).as_deref(), Some("az-b"));
    assert_eq!(zones.zone_of("n02", read).as_deref(), Some("az-b"));
    assert_eq!(reads.get(), 1, "remembered");
    assert_eq!(zones.zone_of("gone", || Ok::<_, ()>(None)), None);
    assert_eq!(
        zones.zone_of("gone", || -> Result<Option<_>, ()> {
            panic!("remembered as none")
        }),
        None
    );
    assert_eq!(
        zones.zone_of("err", || Err::<Option<oss_format::NodeLease>, _>(())),
        None
    );
    assert_eq!(
        zones.zone_of("err", read).as_deref(),
        Some("az-b"),
        "a failure is not remembered"
    );
}

#[test]
fn nobody_live_has_no_writer_and_no_winner() {
    let fleet = fleet();
    let view = FleetView {
        members: &fleet,
        now: NOW + 100 * TTL,
        ttl: TTL,
        local: "n01",
        holding: Holding { held: 0, cap: 0 },
    };
    let key = ShardKey::new("logs", 0);
    assert_eq!(view.writer_for(key), None);
    assert_eq!(view.takeover_winner(key, "n02", 0.0, || None), None);
    assert_eq!(view.fleet_size().wanted, -1);
    assert_eq!(view.fleet_mean_held(), -1.0);
    assert!(!view.nearly_full() && !view.above_fleet_share());
    assert_eq!(view.hand_off_target(), None);
}

/// A lease from a `boundaries.txt` member:
/// `id:roles:zone:held:cap:in_use:refused:expires_delta:revoked`.
fn member(spec: &str) -> oss_format::NodeLease {
    let f: Vec<&str> = spec.split(':').collect();
    let expires = NOW + f[7].parse::<i64>().unwrap();
    let mut lease =
        oss_format::NodeLease::new(f[0], &format!("e{}", f[0]), &format!("a{}", f[0]), expires);
    lease.set_roles(f[1].split('+').map(str::to_owned));
    if f[2] != "-" {
        lease.set_zone(Some(f[2].to_owned()));
    }
    lease.held = f[3].parse().unwrap();
    lease.cap = f[4].parse().unwrap();
    lease.in_use = f[5].parse().unwrap();
    lease.refused = f[6].parse().unwrap();
    lease.revoked = f[8] == "true";
    lease
}

#[test]
fn every_decision_at_its_thresholds_is_the_javas() {
    let departed: HashMap<&str, &str> = [("gone-a", "az-a")].into();
    let rows = rows("boundaries.txt");
    assert_eq!(rows.len(), 400);
    let (mut handoffs, mut above) = (0, 0);
    for (shard, row) in rows.iter().enumerate() {
        let fleet: Vec<_> = row[0].split(';').map(member).collect();
        let view = FleetView {
            members: &fleet,
            now: NOW,
            ttl: TTL,
            local: &row[1],
            holding: Holding {
                held: row[2].parse().unwrap(),
                cap: row[3].parse().unwrap(),
            },
        };
        let s = view.fleet_size();
        assert_eq!(
            format!("{},{},{}", s.members, s.wanted, s.demand),
            row[5],
            "{row:?}"
        );
        assert_eq!(or_dash(view.member_with_room()), row[6], "{row:?}");
        assert_eq!(
            view.fleet_mean_held(),
            row[7].parse::<f64>().unwrap(),
            "{row:?}"
        );
        assert_eq!(or_dash(view.member_below_mean()), row[8], "{row:?}");
        assert_eq!(view.above_fleet_share().to_string(), row[9], "{row:?}");
        assert_eq!(or_dash(view.hand_off_target()), row[10], "{row:?}");
        let key = ShardKey::new("logs", i32::try_from(shard).unwrap());
        assert_eq!(or_dash(view.writer_for(key)), row[11], "{row:?}");
        let dead = row[12].as_str();
        let winner = view.takeover_winner(key, dead, row[4].parse().unwrap(), || {
            departed.get(dead).map(|z| (*z).to_owned())
        });
        assert_eq!(or_dash(winner), row[13], "{row:?}");
        handoffs += usize::from(row[10] != "-");
        above += usize::from(row[9] == "true");
    }
    assert!(handoffs > 20 && above > 20, "{handoffs} {above}");
}

#[test]
fn departed_zones_are_forgotten_all_at_once_at_the_bound() {
    use oss_cluster::placement::DepartedZones;
    use std::cell::Cell;
    let zones = DepartedZones::default();
    let reads = Cell::new(0);
    let read = || {
        reads.set(reads.get() + 1);
        Ok::<Option<oss_format::NodeLease>, ()>(None)
    };
    zones.zone_of("first", read);
    zones.zone_of("second", read);
    zones.zone_of("first", read);
    assert_eq!(reads.get(), 2, "below the bound both are kept");
    for i in 0..4094 {
        zones.zone_of(&format!("d{i}"), read);
    }
    zones.zone_of("first", read);
    assert_eq!(reads.get(), 4096, "4,096 remembered");
    zones.zone_of("one-more", read);
    zones.zone_of("first", read);
    assert_eq!(reads.get(), 4098, "the bound cleared them");
}

#[test]
fn a_writer_at_its_cap_has_no_room_for_writes_or_takeovers() {
    let mut full = member("full:ingest:az-a:4:4:0:0:30000:false");
    let roomy = member("roomy:ingest:az-a:3:4:0:0:30000:false");
    let fleet = vec![full.clone(), roomy];
    let view = FleetView {
        members: &fleet,
        now: NOW,
        ttl: TTL,
        local: "self",
        holding: Holding { held: 0, cap: 0 },
    };
    for shard in 0..64 {
        let key = ShardKey::new("logs", shard);
        assert_eq!(view.writer_for(key), Some("roomy"), "{shard}");
        assert_eq!(
            view.takeover_winner(key, "dead", 0.0, || Some("az-a".to_owned())),
            Some("roomy")
        );
    }
    // Overcommit lets the full one take a share again.
    let over = (0..64)
        .filter(|s| {
            view.takeover_winner(ShardKey::new("logs", *s), "dead", 0.5, || None) == Some("full")
        })
        .count();
    assert!(over > 0);
    // When every writer is full, writes still go somewhere.
    full.held = 9;
    let both_full = vec![
        full.clone(),
        member("other:ingest:az-a:4:4:0:0:30000:false"),
    ];
    let view = FleetView {
        members: &both_full,
        ..view
    };
    assert!(view.writer_for(ShardKey::new("logs", 0)).is_some());
}
