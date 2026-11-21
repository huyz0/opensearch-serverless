# Verified: cardinality of a keyword field

- AC-1: `a_keyword_cardinality_counts_the_distinct_terms_across_segments` in crates/reader/src/aggs/metric/collect_tests.rs
- AC-2: `a_keyword_cardinality_reserves_its_ordinals_and_sketch` in crates/reader/src/aggs/metric/collect_tests.rs
- AC-3: `fields_a_metric_cannot_read_fail_the_shard` in crates/reader/src/aggs/metric/collect_tests.rs
