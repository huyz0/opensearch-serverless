# Verified: the coordinator routes each bulk item to a shard and groups items by shard

- AC-1: `an_item_already_failed_is_not_looked_up` in crates/coordinator/tests/server/bulk_route.rs
- AC-2: `a_missing_or_blank_index_fails_the_item` in crates/coordinator/tests/server/bulk_route.rs
- AC-3: `a_system_index_is_refused_before_any_lookup` in crates/coordinator/tests/server/bulk_route.rs
- AC-4: `lookups_fail_per_index_and_happen_once_per_name` in crates/coordinator/tests/server/bulk_route.rs
- AC-5: `a_data_stream_takes_only_creates` in crates/coordinator/tests/server/bulk_route.rs
- AC-6: `a_routed_item_names_its_backing_index_uuid_and_shard` in crates/coordinator/tests/server/bulk_route.rs
- AC-7: `items_group_by_index_and_shard_in_first_seen_order` in crates/coordinator/tests/server/bulk_route.rs
