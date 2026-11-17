//! The 64-bit hash, declared-shard slices, and every recorded id routed in
//! both shard modes and in a legacy index (AC-1, AC-2, AC-4).

use oss_cluster::placement::murmur3;
use oss_cluster::routing::{
    DocumentRouting, RoutingError, UnitLayout, declared_shard, first_hash_of, last_hash_of,
    murmur3_x64_h1, routing_hash,
};

use crate::{hash, rows, unhex};

const DECLARED: [u32; 8] = [1, 2, 3, 5, 7, 30, 1000, 4096];

#[test]
fn every_routing_hash_and_declared_shard_is_the_javas() {
    let rows = rows("hashes.txt");
    assert_eq!(rows.len(), 10_000);
    assert_eq!(rows[1][1], "cbd8a7b341bd9b02", "the recorded hash of hello");
    for row in &rows {
        let id = unhex(&row[0]);
        let h = routing_hash(&id);
        assert_eq!(h, hash(&row[1]), "hash of {id:?}");
        assert_eq!(murmur3_x64_h1(id.as_bytes()), h);
        assert_eq!(murmur3(&[&id]).to_string(), row[2], "murmur3 of {id:?}");
        for (at, n) in DECLARED.iter().enumerate() {
            assert_eq!(
                declared_shard(h, *n).to_string(),
                row[3 + at],
                "{id:?} in {n}"
            );
        }
    }
}

#[test]
fn every_declared_shard_bound_is_the_javas() {
    let rows = rows("bounds.txt");
    assert!(rows.len() > 1000);
    for row in rows {
        let n: u32 = row[0].parse().unwrap();
        let s: u32 = row[1].parse().unwrap();
        let (first, last) = (hash(&row[2]), hash(&row[3]));
        assert_eq!(first_hash_of(s, n), Some(first), "first of {s} in {n}");
        assert_eq!(last_hash_of(s, n), Some(last), "last of {s} in {n}");
        assert_eq!(declared_shard(first, n), s);
        assert_eq!(declared_shard(last, n), s);
        if s > 0 {
            assert_eq!(
                declared_shard(first - 1, n),
                s - 1,
                "first of {s} in {n} is the least"
            );
        }
    }
    assert_eq!(first_hash_of(3, 3), None);
    assert_eq!(last_hash_of(3, 3), None);
}

#[test]
fn every_legacy_shard_is_cores() {
    let rows = rows("legacy.txt");
    assert_eq!(rows.len(), 10_000);
    let mut partitioned = 0;
    for row in rows {
        let routing =
            DocumentRouting::legacy(row[0].parse().unwrap(), row[1].parse().unwrap()).unwrap();
        let id = unhex(&row[2]);
        let given = (row[3] != "~").then(|| unhex(&row[3]));
        let shard = routing.shard_for(&id, given.as_deref()).unwrap();
        assert_eq!(
            shard.to_string(),
            row[4],
            "{id:?} routed by {given:?} in {row:?}"
        );
        if row[1] != "1" {
            partitioned += 1;
            assert_eq!(
                routing.shard_for(&id, None),
                Err(RoutingError::RoutingMissing)
            );
            assert_eq!(routing.shards_for_search(&["a"]), None);
        }
    }
    assert!(partitioned > 3000);
}

#[test]
fn a_legacy_index_needs_shards_and_a_partition_below_them() {
    assert!(DocumentRouting::legacy(0, 1).is_err());
    assert!(DocumentRouting::legacy(5, 0).is_err());
    assert!(DocumentRouting::legacy(5, 5).is_err());
    assert!(DocumentRouting::legacy(5, 4).is_ok());
    assert!(DocumentRouting::legacy(1, 1).is_ok());
}

#[test]
fn every_id_routes_to_the_javas_unit_in_both_modes() {
    let rows = rows("units.txt");
    let layouts: Vec<UnitLayout> = rows[..3]
        .iter()
        .map(|r| UnitLayout::parse(r[1].as_bytes()).unwrap())
        .collect();
    assert_eq!(rows.len(), 10_003);
    for row in &rows[3..] {
        let id = unhex(&row[0]);
        for (at, layout) in layouts.iter().enumerate() {
            let routing = DocumentRouting::Units(layout);
            assert_eq!(
                routing.shard_for(&id, None).unwrap().to_string(),
                row[1 + at],
                "{id:?}"
            );
            // A routing value replaces the id.
            assert_eq!(
                routing.shard_for("other", Some(&id)).unwrap(),
                layout.unit_for_routing(&id).id
            );
        }
    }
    let fixed = DocumentRouting::Units(&layouts[0]);
    let ids: Vec<String> = rows[3..13].iter().map(|r| unhex(&r[0])).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let shards = fixed.shards_for_search(&refs).unwrap();
    let expected: std::collections::BTreeSet<u32> =
        rows[3..13].iter().map(|r| r[1].parse().unwrap()).collect();
    assert_eq!(shards, expected);
}

#[test]
fn search_routings_split_as_cores() {
    use oss_cluster::routing::{normalize, search_routings};
    assert_eq!(search_routings(Some("a,b,,a,c,")), vec!["a", "b", "c"]);
    assert!(search_routings(None).is_empty());
    assert!(search_routings(Some("")).is_empty());
    assert_eq!(normalize(Some("")), None);
    assert_eq!(normalize(Some("x")), Some("x"));
    assert_eq!(normalize(None), None);
}

#[test]
fn routing_shards_scale_the_hash_as_core_does() {
    // Stock OpenSearch over three shards and 768 routing shards put ids
    // "0" to "7" on these shards (fixtures/compat/bulk/shards.json).
    let routing = DocumentRouting::legacy_scaled(3, 1, 768).unwrap();
    let shards: Vec<u32> = (0..8)
        .map(|i| routing.shard_for(&i.to_string(), None).unwrap())
        .collect();
    assert_eq!(shards, [1, 2, 1, 1, 1, 0, 2, 0]);
    assert_eq!(routing.shard_for("x", Some("same")), Ok(1));
    // Without routing shards the factor is 1, as the shell renders an index.
    assert_eq!(
        DocumentRouting::legacy_scaled(3, 1, 3).unwrap(),
        DocumentRouting::legacy(3, 1).unwrap()
    );
    assert!(DocumentRouting::legacy_scaled(3, 1, 4).is_err());
    assert!(DocumentRouting::legacy_scaled(3, 1, 0).is_err());
    assert!(DocumentRouting::legacy_scaled(4, 2, 2).is_err());
}
