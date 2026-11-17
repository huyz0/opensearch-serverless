# Reader: the cardinality aggregation: verified

Run with `cargo test --workspace --all-features` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container). Recordings from a stock OpenSearch 3.10.0-SNAPSHOT built from upstream's `3.10` branch; the vectors of AC-3 and AC-4 printed by its `HyperLogLogPlusPlus`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the cardinality steps of `paging-sort.json`)
- AC-2: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the cardinality steps of `errors.json`), `malformed_options_fail_as_opensearch_fails_them`, `cardinality_options_the_shard_refuses_fail_it_in_opensearch_order`
- AC-3: `estimates_and_merges_match_opensearch`, `cardinalities_reduce_as_opensearch_reduces_them`
- AC-4: `precision_follows_the_threshold_as_opensearch_computes_it`, `the_precision_follows_the_threshold`
- AC-5: `cardinality_estimates_the_distinct_values_as_opensearch_does`
- AC-6: `a_cardinality_reserves_its_sketch`, `a_cardinality_reserves_its_sketch_only_once_it_can_have_one`, `a_sketch_reserves_five_times_its_registers`
