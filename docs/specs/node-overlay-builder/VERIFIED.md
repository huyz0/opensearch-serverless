# Verified: the node builds overlays in the background

- AC-1: `ready_answers_nothing_at_once_and_the_overlay_once_built` in crates/node/tests/overlay_builder.rs
- AC-2: `unchanged_records_give_the_same_overlay_and_build_nothing_more` in crates/node/tests/overlay_builder.rs
- AC-3: `new_records_build_again_and_the_old_overlay_serves_meanwhile` in crates/node/tests/overlay_builder.rs
- AC-4: `a_pool_that_refuses_builds_nothing_and_a_finished_overlay_holds_its_estimate` in crates/node/tests/overlay_builder.rs
- AC-5: `a_build_that_fails_is_not_retried_until_the_records_change` in crates/node/tests/overlay_builder.rs
- AC-5 (refused, retried): `a_refused_build_is_tried_again_once_the_pool_has_room` in crates/node/tests/overlay_builder.rs
- AC-6: `the_first_ask_tracks_the_shard_at_the_commits_checkpoint_and_closing_untracks_it` in crates/node/tests/overlay_builder.rs and `a_source_is_told_when_the_engine_it_served_closes` in crates/node/tests/overlay_search.rs
- AC-7: `a_full_queue_refuses_a_further_build_and_the_asks_finish_later` in crates/node/tests/overlay_builder.rs
- AC-8: `records_the_commit_holds_are_left_out_and_newer_ones_kept` in crates/reader/tests/overlay_build_ready.rs
- AC-9: `a_build_queued_for_an_old_stage_is_dropped_when_the_stage_changed` in crates/node/tests/overlay_builder.rs
- AC-10: `an_overlay_is_charged_its_estimate_and_the_masks_of_the_commit` in crates/node/tests/overlay_builder.rs
