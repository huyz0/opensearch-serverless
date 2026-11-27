# Verified: index-blocks

- AC-1: `a_full_block_refuses_every_item_and_writes_nothing` in crates/writer/tests/it/bulk_write_block.rs, `the_routes_read_the_write_blocks_flat_nested_bool_or_text` in crates/node/tests/roles/descriptor_routes.rs
- AC-2: `an_allow_delete_block_refuses_all_but_a_delete` in crates/writer/tests/it/bulk_write_block.rs, `an_owner_refuses_the_items_a_write_block_stops` in crates/node/tests/roles/bulk_handler.rs
- AC-3: `a_write_block_set_through_the_route_is_what_a_bulk_reads_and_false_lifts_it` in crates/node/tests/server/settings_route.rs
- AC-4: `a_write_block_set_through_the_route_is_what_a_bulk_reads_and_false_lifts_it` in crates/node/tests/server/settings_route.rs
- AC-5: `a_refused_update_changes_nothing` in crates/node/tests/server/settings_route.rs
- AC-6: `a_lookup_is_one_read_and_no_list` in crates/node/tests/roles/descriptor_routes.rs
