# Janitor: sweep what a node has just published: verified

Run with `cargo test -p oss-janitor`, `cargo mutants -p oss-janitor` (`published/mod.rs`: 28 mutants, 25 caught, 3 unviable, none missed; `gc/memory.rs` unchanged apart from `forget`, covered by the forget test) and the checks listed in the PR, on 2026-11-19 (Linux sandbox, emulator only, numbers provisional).

- AC-1: `a_first_publish_is_swept_and_an_add_only_one_after_it_is_not`
- AC-2: `a_dropped_file_triggers_a_sweep_and_a_lost_segments_file_does_not`, `a_failed_sweep_of_a_dropping_publish_is_tried_again`, `a_skipped_add_then_a_merge_that_drops_it_is_swept`
- AC-2b: `a_file_dropped_in_the_live_term_is_swept_when_the_term_moves_on`
- AC-3: `a_watched_shard_is_swept_without_being_published_until_nothing_is_watched`, `a_shard_both_published_and_watched_is_swept_once`
- AC-4: `force_makes_the_next_pass_sweep_the_shard_once`
- AC-5: `forget_resets_the_state_and_the_memory`
- AC-6: `an_empty_pass_sends_no_request`
- AC-7: `a_failing_shard_is_reported_and_the_others_swept_and_it_is_tried_again`, `a_shard_that_cannot_name_a_path_is_reported`, `a_failed_sweep_is_tried_again_though_nothing_is_published`
- AC-8: `a_pass_costs_the_sweeps_it_runs_and_nothing_for_the_shards_it_skips`
