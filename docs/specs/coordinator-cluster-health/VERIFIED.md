# Verified: the coordinator judges cluster health from what the shard heads say

- AC-1: `node_conditions_parse_and_compare` in crates/coordinator/tests/cluster_health.rs
- AC-2: `refusals_come_in_the_shells_order_and_words` in crates/coordinator/tests/cluster_health.rs
- AC-3: `unreadable_things_are_red_and_a_missing_index_ends_it` in crates/coordinator/tests/cluster_health.rs
- AC-4: `shards_are_owned_dormant_or_unservable` in crates/coordinator/tests/cluster_health.rs
- AC-5: `counts_add_up_and_per_shard_states_are_optional` in crates/coordinator/tests/cluster_health.rs
- AC-6: `a_wait_is_satisfied_by_green_enough_nodes_or_a_missing_index` in crates/coordinator/tests/cluster_health.rs
- AC-3 (the words): `a_status_is_reported_as_its_word` in crates/coordinator/tests/cluster_health.rs
