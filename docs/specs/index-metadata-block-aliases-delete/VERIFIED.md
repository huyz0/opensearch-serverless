# Verified: index-metadata-block-aliases-delete

- AC-1: `an_alias_add_naming_a_blocked_index_is_refused` in crates/node/tests/server/metadata_block.rs
- AC-2: `an_alias_remove_naming_a_blocked_index_is_refused` in crates/node/tests/server/metadata_block.rs
- AC-3: `a_blocked_index_among_several_actions_stops_them_all` in crates/node/tests/server/metadata_block.rs
- AC-4: `a_blocked_index_is_not_deleted_until_the_block_is_lifted` in crates/node/tests/server/metadata_block.rs
- AC-5: `a_missing_index_and_an_unblocked_one_answer_as_before` in crates/node/tests/server/metadata_block.rs
- AC-6: `a_rollover_of_a_blocked_index_is_refused_and_changes_nothing` in crates/node/tests/server/metadata_block.rs
- AC-7: `a_data_stream_with_a_blocked_backing_index_is_not_deleted` in crates/node/tests/server/metadata_block.rs
