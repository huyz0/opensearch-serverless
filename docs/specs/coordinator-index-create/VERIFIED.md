# Verified: the coordinator reads a create-index request

- AC-1: `no_body_is_the_query_shard_count` in crates/coordinator/tests/index_create.rs
- AC-2: `a_body_that_is_not_an_object_is_malformed` in crates/coordinator/tests/index_create.rs
- AC-3: `a_body_without_an_envelope_is_the_mapping` in crates/coordinator/tests/index_create.rs
- AC-4: `envelope_parts_must_be_objects` in crates/coordinator/tests/index_create.rs
- AC-5: `settings_are_flattened_prefixed_and_sorted` in crates/coordinator/tests/index_create.rs
- AC-6: `replicas_other_than_zero_are_refused` in crates/coordinator/tests/index_create.rs
- AC-7: `the_shard_count_comes_from_settings_or_the_query_within_bounds` in crates/coordinator/tests/index_create.rs
- AC-8: `shard_and_replica_counts_are_consumed` in crates/coordinator/tests/index_create.rs
- AC-9: `alias_options_are_kept_and_unsupported_ones_refused` in crates/coordinator/tests/index_create.rs
