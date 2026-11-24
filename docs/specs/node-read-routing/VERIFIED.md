# Verified: node-read-routing

- AC-1: `a_get_of_a_shard_another_node_owns_is_answered_by_that_owner` in crates/node/tests/server/get_route.rs
- AC-2: `a_remote_refresh_counts_the_owners_shard_and_an_unreachable_owner_fails_it` in crates/node/tests/server/get_route.rs
- AC-3: `a_remote_refresh_counts_the_owners_shard_and_an_unreachable_owner_fails_it` in crates/node/tests/server/get_route.rs
- AC-4: `a_get_of_a_shard_another_node_owns_is_asked_of_that_owner` in crates/node/tests/roles/cluster_owner.rs
- AC-5: `a_write_through_one_node_reaches_the_node_that_owns_the_shard` in crates/node/tests/server/boot.rs
