# Verified: a node that starts from configuration

- AC-1: `what_cannot_be_used_is_refused_by_name` in crates/node/src/boot/config.rs
- AC-2: `the_secret_is_not_printed` in crates/node/src/boot/config.rs
- AC-3: `a_started_node_is_enrolled_and_reports_itself` in crates/node/tests/server/boot.rs
- AC-4: `a_started_node_writes_and_reads_a_document` in crates/node/tests/server/boot.rs
- AC-5: `a_second_node_is_seen_and_a_dropped_one_is_released` in crates/node/tests/server/boot.rs
- AC-6: `a_second_node_is_seen_and_a_dropped_one_is_released` in crates/node/tests/server/boot.rs
- AC-7: `a_restarted_node_writes_above_the_epochs_it_left` in crates/node/tests/server/boot.rs
- AC-8: `a_write_through_one_node_reaches_the_node_that_owns_the_shard` in crates/node/tests/server/boot.rs
- AC-9: `a_crashed_node_leaves_its_lease_to_lapse` in crates/node/tests/server/boot.rs
