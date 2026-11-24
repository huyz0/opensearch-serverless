# Verified: node retire

- AC-1: `a_retired_node_leaves_its_shard_to_the_next_writer_at_once` in crates/node/tests/server/boot_retire.rs
- AC-2: `a_retired_node_leaves_its_shard_to_the_next_writer_at_once` in crates/node/tests/server/boot_retire.rs
- AC-3: `retiring_publishes_and_gives_back_the_head_and_a_second_retire_asks_nothing` in crates/node/tests/writer_retire.rs
- AC-4: `retiring_gives_back_more_heads_than_one_pass_may` in crates/node/tests/writer_retire.rs
- AC-5: `a_retired_node_leaves_its_shard_to_the_next_writer_at_once` in crates/node/tests/server/boot_retire.rs
- AC-6: `a_node_holding_nothing_retires_with_nothing_to_do` in crates/node/tests/server/boot_retire.rs
- AC-7: `the_binary_retires_on_sigterm_and_exits_zero` in crates/node/tests/server/boot_retire.rs
