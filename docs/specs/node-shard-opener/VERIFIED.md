# Verified: node-shard-opener

- AC-1: `a_first_write_opens_the_shard_and_a_second_opens_nothing` in crates/node/tests/writer_opener.rs
- AC-2: `a_first_write_opens_the_shard_and_a_second_opens_nothing` in crates/node/tests/writer_opener.rs
- AC-3: `groups_racing_for_an_unheld_shard_open_it_once` in crates/node/tests/writer_opener.rs
- AC-4: `a_shard_a_live_node_holds_is_not_opened_here_and_its_write_goes_to_that_node` in crates/node/tests/writer_opener.rs
- AC-5: `a_shard_with_a_history_is_refused_as_takeover` in crates/node/tests/writer_opener.rs
- AC-6: `an_index_recreated_since_the_group_was_planned_is_not_opened` in crates/node/tests/writer_opener.rs
- AC-7: `a_role_without_an_opener_says_the_shard_is_not_open` in crates/node/tests/writer_opener.rs
