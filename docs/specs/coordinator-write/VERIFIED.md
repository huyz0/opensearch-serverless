# Verified: the coordinator decides where a write is applied and which native write options it accepts

- AC-1: `a_held_shard_is_written_here_whoever_the_head_names` in crates/coordinator/tests/write.rs
- AC-2: `an_unheld_shard_nobody_or_this_node_owns_is_activated_first` in crates/coordinator/tests/write.rs
- AC-3: `after_the_activation_was_tried_the_refusal_says_why` in crates/coordinator/tests/write.rs
- AC-4: `another_nodes_shard_is_forwarded_without_activating` in crates/coordinator/tests/write.rs
- AC-5: `only_the_same_incarnation_settles_after_an_activation` in crates/coordinator/tests/write.rs
- AC-6: `native_writes_accept_only_what_the_log_can_honour` in crates/coordinator/tests/write.rs
