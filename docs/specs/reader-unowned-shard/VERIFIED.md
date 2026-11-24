# Verified: a reader serves a shard whose writer let it go

- AC-1: `a_writer_that_does_not_hold_the_shard_and_a_released_head_let_a_search_answer` in crates/node/tests/tail/tail_fresh.rs
- AC-2: `a_writer_that_does_not_hold_the_shard_and_an_owned_head_still_refuse` in crates/node/tests/tail/tail_fresh.rs
- AC-3: `the_head_is_read_once_for_searches_within_the_keep` in crates/node/tests/tail/tail_fresh.rs
- AC-4: `with_no_head_reader_a_writer_that_does_not_hold_the_shard_refuses` in crates/node/tests/tail/tail_fresh.rs
- AC-5: `a_fleet_that_gains_an_empty_node_with_hand_off_off` in crates/node/tests/hand_off_evidence.rs
