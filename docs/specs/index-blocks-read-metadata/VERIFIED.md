# Verified: index-blocks-read-metadata

- AC-1: `an_index_with_a_read_block_does_not_open` in crates/reader/src/settings_tests.rs, `a_search_of_an_index_with_a_read_block_is_forbidden_and_lifting_it_opens_it` in crates/node/tests/server/read_block.rs
- AC-2: `a_search_of_an_index_with_a_read_block_is_forbidden_and_lifting_it_opens_it` in crates/node/tests/server/read_block.rs
- AC-3: `a_metadata_block_refuses_a_settings_change_but_not_lifting_itself` in crates/node/tests/server/settings_route.rs
- AC-4: `a_refused_update_changes_nothing` in crates/node/tests/server/settings_route.rs
- AC-5: `a_metadata_blocked_index_among_several_refuses_before_any_write` in crates/node/tests/server/settings_route.rs
