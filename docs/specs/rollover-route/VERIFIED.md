# Verified

- AC-1: `crates/node/tests/server/rollover_route.rs` `a_stream_rolls_over_to_the_next_generation`
- AC-2: `crates/node/tests/server/rollover_route.rs` `conditions_decide_whether_it_rolls`
- AC-3: `crates/node/tests/server/rollover_route.rs` `the_new_backing_index_has_the_templates_mapping_and_a_date_timestamp`, `a_taken_backing_name_is_moved_past_by_a_generation`
- AC-4: `crates/node/tests/server/rollover_route.rs` `an_alias_with_a_write_index_gains_the_new_one_as_its_write_index`, `an_alias_of_one_index_with_no_write_index_moves_to_the_new_one`
- AC-5: `crates/node/tests/server/rollover_route.rs` `what_cannot_roll_is_refused_and_creates_nothing`
- AC-6: `crates/node/tests/server/rollover_route.rs` `the_requests_are_the_budget_rows`
- AC-7: `crates/node/tests/server/rollover_route.rs` `a_swap_lost_to_another_change_removes_the_index_it_made_and_says_so`
