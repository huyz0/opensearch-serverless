# Reader: metric aggregations: verified

Run with `cargo test --workspace --all-features` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container). Recordings from a stock OpenSearch 3.10.0-SNAPSHOT built from upstream's `3.10` branch.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the metric steps of `paging-sort.json`)
- AC-2: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (the aggregation steps of `errors.json`), `malformed_aggregations_fail_as_opensearch_fails_them`, `a_parse_failure_carries_its_exception_and_location`, `fields_a_metric_cannot_read_fail_the_shard`
- AC-3: `metrics_of_every_match_across_segments`, `metrics_of_the_query_matches_only`, `an_unmapped_field_or_no_match_aggregates_nothing`
- AC-4: `names_are_listed_as_opensearch_listed_them`, `names_are_listed_as_a_hash_map_iterates_them`, `a_thirteenth_name_doubles_the_table`, `results_are_listed_in_hash_map_order`
- AC-5: `shard_results_reduce_as_opensearch_reduces_them`
- AC-6: `a_cold_search_with_metric_aggregations_then_a_warm_one`
- AC-7: `a_search_reserves_its_aggregation_state_with_its_hits`, `state_bytes_grow_with_each_aggregation`
- AC-8: `doubles_print_as_java_prints_them`, `dates_print_in_the_default_format`, `a_double_is_cast_as_java_casts_it`, `results_are_written_as_opensearch_writes_them`
