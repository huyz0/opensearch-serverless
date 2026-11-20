# Point in time routes: verified

Run on 2026-11-19 in a Linux sandbox, emulator numbers provisional: `cargo test -p oss-node --features writer --test pit_route`.

- AC-1: `a_create_freezes_every_shard_and_answers_the_view`.
- AC-2: `keep_alive_takes_time_units_and_is_refused_past_an_hour`.
- AC-3: `what_cannot_be_frozen_is_refused_with_the_javas_status`.
- AC-4: `a_delete_by_path_body_and_all_releases_and_says_which` and `a_delete_that_names_nothing_usable_is_refused_not_failed`.
- AC-5: `all_leaves_a_capture_in_progress_and_the_listing_hides_it`.
- AC-6: `a_list_gives_each_finished_view_its_remaining_life_and_drops_the_expired`.
- AC-7: `the_literal_all_wins_over_an_id_in_the_path`.
- AC-8: `time_values_take_every_unit_and_overflow_is_refused` and `empty_routing_and_preference_are_no_refusal_and_both_spellings_work`.
- AC-9: `a_delete_body_is_bounded_deduplicated_and_strict`.
- AC-10: `a_request_the_body_pool_cannot_hold_a_buffer_for_is_refused_with_429` and `a_view_that_does_not_fit_the_buffer_fails_cleanly_and_pins_nothing`.
- AC-11: `requests_are_what_the_spec_says_and_a_refusal_sends_none`.

Local mutation sweep of `pit_route` was not completed: the container restarted twice during `cargo mutants`. CI's sharded sweep is the evidence for it.
