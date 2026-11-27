# Verified: `typed_keys` on `_search`, metric aggregations

- AC-1: `typed_keys_prefixes_a_metric_aggregation_with_its_type` in crates/node/tests/it/search_url_params.rs (seen red as an unrecognized parameter, green after)
- AC-2: `typed_keys_over_several_indexes_prefixes_the_merged_aggregations` in crates/node/tests/it/search_url_params.rs
- AC-3: `typed_keys_without_aggregations_changes_nothing_and_a_non_boolean_is_refused` in crates/node/tests/it/search_url_params.rs
