# Verified: a multi-node cluster suite

- AC-1: `a_bulk_through_any_node_lands_on_the_node_that_owns_each_shard` in crates/node/tests/cluster_suite.rs
- AC-2: `a_bulk_through_any_node_lands_on_the_node_that_owns_each_shard` in crates/node/tests/cluster_suite.rs
- AC-3: `a_search_from_any_node_finds_every_document` in crates/node/tests/cluster_suite.rs
- AC-4: `the_others_take_over_a_killed_nodes_shards_with_its_documents` in crates/node/tests/cluster_suite.rs
- AC-5: `the_same_scenario_runs_on_rustfs` in crates/node/tests/cluster_suite.rs
