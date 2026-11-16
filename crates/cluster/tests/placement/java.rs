//! Hashes, writer owners, reader ranks and zone preference against the
//! Java's answers (AC-1 to AC-4).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use oss_cluster::placement::{
    HotShardSpread, ShardKey, SpreadConfig, ZonePreference, java_hash_code, murmur3, owner,
    readers_for,
};

use crate::{fleet, list, rows};

fn unhex(text: &str) -> String {
    if text == "-" {
        return String::new();
    }
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn murmur3_and_hash_code_are_the_javas() {
    let rows = rows("hashes.txt");
    assert_eq!(rows.len(), 14);
    for row in rows {
        let s = unhex(&row[0]);
        assert_eq!(murmur3(&[&s]).to_string(), row[1], "murmur3 of {s:?}");
        assert_eq!(
            java_hash_code(&[&s]).to_string(),
            row[2],
            "hashCode of {s:?}"
        );
        // Parts hash as their join.
        let mid = s
            .char_indices()
            .nth(s.chars().count() / 2)
            .map_or(s.len(), |(i, _)| i);
        let (a, b) = s.split_at(mid);
        assert_eq!(murmur3(&[a, b]), murmur3(&[&s]));
        assert_eq!(java_hash_code(&[a, "", b]), java_hash_code(&[&s]));
    }
}

#[test]
fn every_writer_owner_is_the_javas() {
    let rows = rows("owners.txt");
    assert_eq!(rows.len(), 80);
    for row in rows {
        let (index, shard) = row[0].rsplit_once('/').unwrap();
        let key = ShardKey::new(index, shard.parse().unwrap());
        assert_eq!(owner(key, list(&row[1])), Some(row[2].as_str()), "{row:?}");
    }
    assert_eq!(owner(ShardKey::new("logs", 0), []), None);
}

#[test]
fn every_reader_rank_is_the_javas() {
    let fleet = fleet();
    let rows = rows("readers.txt");
    assert_eq!(rows.len(), 180);
    for row in rows {
        let key = ShardKey::new(&row[0], row[1].parse().unwrap());
        let ranked = readers_for(key, &fleet, &row[2], row[3].parse().unwrap());
        assert_eq!(ranked, list(&row[4]), "{row:?}");
    }
}

#[test]
fn every_zone_preference_is_the_javas() {
    let fleet = fleet();
    let rows = rows("zones.txt");
    assert_eq!(rows.len(), 360);
    let mut moved = 0;
    for row in rows {
        let key = ShardKey::new(&row[0], row[1].parse().unwrap());
        let clock = Arc::new(AtomicI64::new(1_000_000_000_000));
        let nanos = Arc::clone(&clock);
        let spread = HotShardSpread::new(
            SpreadConfig {
                target_per_second: 50.0,
                max_readers: 4,
            },
            Arc::new(move || nanos.load(Ordering::Relaxed)),
        )
        .unwrap();
        for _ in 0..row[3].parse::<usize>().unwrap() {
            spread.order(key, &[]);
        }
        let ordered = readers_for(key, &fleet, "search", 2);
        assert_eq!(ordered, list(&row[5]), "{row:?}");
        let busy: Vec<_> = ordered
            .first()
            .map(|id| {
                (0..row[4].parse::<usize>().unwrap())
                    .map(|_| spread.begin(id))
                    .collect()
            })
            .unwrap_or_default();
        let zone = (row[2] != "-").then_some(row[2].as_str());
        let chosen =
            ZonePreference::default().order(key, &ordered, &fleet, "search", zone, &spread);
        assert_eq!(chosen, list(&row[6]), "{row:?}");
        moved += usize::from(chosen != ordered);
        drop(busy);
    }
    assert!(moved > 50, "the recording exercises the move: {moved}");
}

#[test]
fn equal_weights_go_to_the_lower_id_in_javas_order() {
    // Two ids whose weights for one key collide, found by search: the
    // weights are 32 bits, so a few hundred thousand ids hold a pair.
    let mut seen = std::collections::HashMap::new();
    let (a, b) = (0..1_000_000)
        .find_map(|i| {
            let id = format!("n{i}");
            let w = murmur3(&[&id, "/", "logs", "/", "0"]);
            seen.insert(w, id.clone()).map(|other| (other, id))
        })
        .expect("a collision");
    let low = if oss_format::java_order(&a, &b).is_lt() {
        &a
    } else {
        &b
    };
    let key = ShardKey::new("logs", 0);
    assert_eq!(owner(key, [a.as_str(), b.as_str()]), Some(low.as_str()));
    assert_eq!(owner(key, [b.as_str(), a.as_str()]), Some(low.as_str()));
}

#[test]
fn a_zone_local_reader_is_chosen_until_it_runs_more_than_the_spill_beyond() {
    let fleet = fleet();
    let key = ShardKey::new("logs", 0);
    let ordered = readers_for(key, &fleet, "search", 2);
    let clock = Arc::new(AtomicI64::new(0));
    let nanos = Arc::clone(&clock);
    let spread = HotShardSpread::new(
        SpreadConfig::default(),
        Arc::new(move || nanos.load(Ordering::Relaxed)),
    )
    .unwrap();
    spread.order(key, &[]);
    let preference = ZonePreference::new(1.0, 8).unwrap();
    let chosen = preference.order(key, &ordered, &fleet, "search", Some("az-a"), &spread);
    let local = chosen[0];
    assert_ne!(
        local, ordered[0],
        "the recording moves this one: {chosen:?}"
    );
    let first: Vec<_> = (0..2).map(|_| spread.begin(ordered[0])).collect();
    let mut running: Vec<_> = (0..10).map(|_| spread.begin(local)).collect();
    // 10 running against 2: exactly the spill of 8 beyond, still chosen.
    assert_eq!(
        preference.order(key, &ordered, &fleet, "search", Some("az-a"), &spread)[0],
        local
    );
    running.push(spread.begin(local));
    assert_eq!(
        preference.order(key, &ordered, &fleet, "search", Some("az-a"), &spread),
        ordered
    );
    drop((first, running));
}
