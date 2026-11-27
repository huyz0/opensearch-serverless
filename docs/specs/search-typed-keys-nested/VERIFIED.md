# Verified: `typed_keys`: `nested`, `reverse_nested` and `missing`

- AC-1: `typed_keys_tells_missing_nested_and_reverse_nested_from_filter` in crates/node/tests/it/search_url_params.rs (seen red as a 400 for a nested aggregation, green after)
- AC-2: `every_value_has_the_type_its_java_class_writes` in crates/reader/src/aggs/metric/typed.rs and `results_of_different_kinds_do_not_reduce_together` in crates/reader/src/aggs/bucket/filters_tests.rs
