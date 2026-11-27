# Verified: a writer node serves `_refresh` and the document get routes

- AC-1: `a_get_sees_a_write_no_refresh_has_made_visible` in crates/node/tests/server/get_route.rs
- AC-2: `a_deleted_or_unknown_id_is_not_found_and_head_has_no_body` in crates/node/tests/server/get_route.rs
- AC-3: `source_false_leaves_the_source_out_and_other_values_are_refused` in crates/node/tests/server/get_route.rs
- AC-4: `the_source_route_answers_the_source_alone` in crates/node/tests/server/get_route.rs
- AC-5: `an_unknown_index_is_not_found_and_routing_picks_the_shard` in crates/node/tests/server/get_route.rs
- AC-6: `refresh_makes_every_shard_visible_and_counts_them` in crates/node/tests/server/get_route.rs
- AC-7: `a_remote_refresh_counts_the_owners_shard_and_an_unreachable_owner_fails_it` in crates/node/tests/server/get_route.rs
- AC-8: `a_realtime_get_sees_a_write_no_refresh_has_made_visible`, `a_get_that_is_not_realtime_reads_the_last_refresh`, `a_deleted_or_unknown_id_is_not_found` and `a_realtime_get_of_an_id_the_map_does_not_hold_does_not_refresh`, `a_realtime_get_of_a_deleted_id_does_not_refresh` in crates/writer/tests/it/shard_get.rs
