# Verified: node-read-forward

- AC-1: `a_forwarded_get_returns_an_acknowledged_unpublished_write` in crates/node/tests/server/read_forward.rs
- AC-2: `a_forwarded_get_of_a_missing_id_is_not_found` in crates/node/tests/server/read_forward.rs
- AC-3: `a_get_of_an_unknown_index_a_stale_incarnation_or_a_shard_not_held_fails` in crates/node/tests/server/read_forward.rs
- AC-4: `a_get_to_a_node_no_lease_names_sends_nothing` in crates/node/tests/server/read_forward.rs
