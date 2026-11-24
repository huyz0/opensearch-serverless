# Verified: the writer keeps acknowledged writes until a commit holds them

- AC-1: `acked_records_come_back_oldest_first_and_only_those_above` in crates/node/tests/tail/tail_keep.rs
- AC-2: `a_publish_drops_what_it_holds_and_a_later_ack_below_it_is_not_kept` in crates/node/tests/tail/tail_keep.rs
- AC-3: `the_floor_is_the_lowest_first_ack_minus_one` in crates/node/tests/tail/tail_keep.rs
- AC-4: `past_the_shard_bounds_the_oldest_go_and_the_reader_is_told_of_a_gap` in crates/node/tests/tail/tail_keep.rs
- AC-5: `past_the_node_bound_a_record_is_not_kept_and_bytes_return_on_release` in crates/node/tests/tail/tail_keep.rs
- AC-8: `a_reader_below_what_was_published_is_told_of_a_gap` in crates/node/tests/tail/tail_keep.rs
- AC-6: `a_write_not_to_be_pushed_begins_the_keep_and_stores_nothing` in crates/node/tests/tail/tail_keep.rs
- AC-7: `a_released_shard_keeps_nothing_and_a_new_ack_begins_a_new_floor` in crates/node/tests/tail/tail_keep.rs
