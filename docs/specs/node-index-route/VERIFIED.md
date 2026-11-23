# Verified: node-index-route

- AC-1: `a_create_writes_the_descriptor_and_acknowledges` in crates/node/tests/server/index_route.rs
- AC-2: `a_second_create_of_a_name_is_refused_and_changes_nothing` in crates/node/tests/server/index_route.rs
- AC-3: `a_bad_name_is_refused_before_any_request` in crates/node/tests/server/index_route.rs
- AC-4: `what_cannot_be_created_writes_nothing` in crates/node/tests/server/index_route.rs
- AC-5: `head_says_whether_a_name_holds_an_index_or_an_alias` in crates/node/tests/server/index_route.rs
- AC-6: `a_create_is_one_conditional_create_and_a_head_one_read` in crates/node/tests/server/index_route.rs
- AC-7: `a_pool_without_room_for_the_buffer_answers_429_and_writes_nothing` in crates/node/tests/server/index_route.rs
- AC-8: `a_body_of_one_mebibyte_is_read_and_one_byte_more_is_refused` in crates/node/tests/server/index_route.rs
- AC-9: `a_descriptor_that_does_not_fit_its_buffer_is_refused_not_failed` in crates/node/tests/server/index_route.rs
