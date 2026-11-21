# Verified: node-cluster-visibility

- AC-1: `a_register_never_written_leaves_nothing_in_force`, `what_the_register_holds_is_put_in_force_once_per_generation` and `a_removed_key_goes_and_the_rest_stay` in crates/node/src/cluster_live.rs
- AC-2: `the_cluster_value_wins_when_it_names_a_mode` in crates/node/src/cluster_live.rs
- AC-3: `a_running_check_follows_the_mode_set_last` in crates/node/tests/tail_fresh.rs
- AC-4: `a_mode_set_on_the_handle_reaches_the_writer_and_the_reader_together` in crates/node/tests/tail_node.rs
- AC-5: `what_cannot_be_used_is_refused_by_name` and `a_file_store_node_takes_the_defaults` in crates/node/src/boot/config.rs
- AC-6: `a_booted_node_follows_the_cluster_registers_refresh_visibility` in crates/node/tests/boot.rs (a PUT of the cluster settings to each mode in turn, then the key removed, each followed within the 50 ms poll the test sets); with the mode no longer applied it fails (times out), observed
- AC-7: `a_booted_node_follows_the_cluster_registers_refresh_visibility` in crates/node/tests/boot.rs (last lines: a node with the overlay off has no feed, `Node::refresh_visibility` is `None`, and the code returns before a store is touched); the read's cost is the row `cluster_settings.read.requests` held by `a_read_of_the_register_is_one_request` in crates/node/tests/cluster_live.rs
