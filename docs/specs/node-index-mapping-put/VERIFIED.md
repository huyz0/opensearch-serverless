# PUT /{index}/_mapping on the node: verification

- AC-1: `an_update_adds_a_field_and_moves_the_version_by_one`, `an_index_with_no_mapping_takes_the_first_one` (node, `tests/server/index_route/mapping_put.rs`).
- AC-2: `an_update_that_changes_nothing_writes_nothing` (node, `tests/server/index_route/mapping_put.rs`).
- AC-3: `a_conflicting_or_unreadable_body_is_refused_and_writes_nothing` (node, `tests/server/index_route/mapping_put.rs`).
- AC-4: `a_missing_body_or_a_name_that_is_not_an_index_is_refused` (node, `tests/server/index_route/mapping_put.rs`).
- AC-5: `a_swap_lost_to_another_update_reads_again_and_keeps_both_fields`, `four_lost_swaps_are_a_conflict` (node, `src/index_route/mapping.rs`).
- AC-6: `an_update_that_changes_nothing_writes_nothing` (node, `tests/server/index_route/mapping_put.rs`).
- AC-7: `updating_a_mapping_needs_the_mapping_put_action`, `every_registered_route_has_an_action_row` (node, `tests/server/security_authz.rs`).
- AC-8: `a_body_of_one_mebibyte_is_read_and_one_byte_more_is_refused` (node, `tests/server/index_route/mapping_put.rs`).
