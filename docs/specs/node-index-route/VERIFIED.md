# Verified: node-index-route

- AC-1: `a_create_writes_the_descriptor_and_acknowledges` in crates/node/tests/index_route.rs
- AC-2: `a_second_create_of_a_name_is_refused_and_changes_nothing` in crates/node/tests/index_route.rs
- AC-3: `a_bad_name_is_refused_before_any_request` in crates/node/tests/index_route.rs
- AC-4: `what_cannot_be_created_writes_nothing` in crates/node/tests/index_route.rs
- AC-5: `head_says_whether_a_name_holds_an_index_or_an_alias` in crates/node/tests/index_route.rs
- AC-6: `a_create_is_one_conditional_create_and_a_head_one_read` in crates/node/tests/index_route.rs
- AC-7: `a_pool_without_room_for_the_buffer_answers_429_and_writes_nothing` in crates/node/tests/index_route.rs
