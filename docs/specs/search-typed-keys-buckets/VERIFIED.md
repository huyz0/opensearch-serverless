# Verified: `typed_keys` on `_search`, bucket aggregations

- AC-1: `typed_keys_prefixes_a_bucket_aggregation_and_its_sub_aggregations` in crates/node/tests/it/search_url_params.rs (seen red as a 400 for a bucket aggregation, green after)
- AC-2: `typed_keys_over_several_indexes_prefixes_a_merged_bucket_aggregation` in crates/node/tests/it/search_url_params.rs
- AC-3: `typed_keys_without_aggregations_changes_nothing_and_a_bucket_one_is_refused` in crates/node/tests/it/search_url_params.rs
