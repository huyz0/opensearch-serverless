# Verified: shard head

- AC-1: `every_case_reads_and_writes_as_the_javas` in crates/format/tests/shard_heads.rs
- AC-2: `truncated_and_corrupt_heads_are_refused` in crates/format/tests/shard_heads.rs
- AC-3: `a_read_is_one_get_and_an_absent_head_is_none` in crates/cluster/tests/leases/heads.rs
- AC-4: `an_absent_head_is_created_at_term_one_with_one_create` in crates/cluster/tests/leases/heads.rs
- AC-5: `a_live_owner_keeps_the_shard_and_nothing_is_written` in crates/cluster/tests/leases/heads.rs
- AC-6: `a_gone_owners_shard_is_taken_at_the_next_term_naming_it_a_writer` in crates/cluster/tests/leases/heads.rs
- AC-7: `one_of_two_racing_nodes_wins_an_absent_head` in crates/cluster/tests/leases/heads.rs
- AC-8: `admission_passes_the_owner_at_its_term_and_refuses_everything_else` in crates/node/tests/writer_heads.rs
- AC-9: `the_first_write_after_a_take_costs_what_the_table_says` in crates/node/tests/writer_first.rs
- AC-10: `a_writer_reads_the_incarnation_once_a_window_and_refuses_a_replaced_index` in crates/node/tests/writer_heads.rs
