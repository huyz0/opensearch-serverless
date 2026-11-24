//! A partition-based `include` of a keyword `terms`, run on the index of
//! four segments.
//!
//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use super::keyword_collect_tests::{shard, terms};

// AC-1
#[test]
fn a_partition_include_keeps_the_terms_whose_murmur_hash_falls_in_it() {
    let shard = shard("kw-partition", 200);
    // The Java reference: `floorMod(murmurhash3_x86_32(term, 31), 4)` of
    // t0 to t15, computed with the standard algorithm.
    let want: [&[&str]; 4] = [
        &["t5", "t9"],
        &["t10", "t11", "t13", "t15"],
        &["t12", "t2", "t3", "t6", "t7", "t8"],
        &["t0", "t1", "t14", "t4"],
    ];
    let mut seen = 0;
    for (partition, terms_of) in want.iter().enumerate() {
        let got = terms(
            &shard,
            &format!(
                r#"{{"aggs":{{"t":{{"terms":{{"field":"tag","size":20,"order":{{"_key":"asc"}},
                "include":{{"partition":{partition},"num_partitions":4}}}}}}}}}}"#
            ),
        );
        let held: Vec<&str> = got.buckets.iter().map(|b| b.0.as_str()).collect();
        assert_eq!(held, *terms_of, "partition {partition}");
        seen += held.len();
    }
    assert_eq!(seen, 16, "the partitions cover every term once");
}
