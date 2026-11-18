# Reader: take a commit its writer pushed: verification

- AC-1: `a_pushed_commit_is_found_with_the_descriptor_read_alone`, `the_engine_opens_a_pushed_commit_with_one_whole_read` (reader, `tests/it/push.rs`).
- AC-2: `a_push_is_kept_until_taken_and_taken_once` (reader, `tests/commit_push.rs`).
- AC-3: `a_push_older_than_the_keep_time_is_not_handed_out` (reader, `tests/commit_push.rs`).
- AC-4: `only_a_newer_push_replaces_the_one_held`, `a_higher_lucene_generation_is_newer_when_no_stage_orders_them` (reader, `tests/commit_push.rs`).
- AC-5: `bytes_that_are_not_a_commit_are_refused_and_held_nowhere`, `a_push_that_is_not_a_commit_is_refused_after_the_descriptor` (reader, `tests/commit_push.rs` and `tests/it/push.rs`).
- AC-6: `past_the_shard_or_byte_bound_a_push_is_refused_and_the_held_stay`, `a_full_mailbox_makes_room_by_dropping_what_is_past_its_keep_time` (reader, `tests/commit_push.rs`).
- AC-7: `a_commit_is_staged_after_another_only_by_one_writer_in_one_term`, `the_newer_of_the_register_and_a_staged_commit_is_picked`, `a_commit_is_newer_by_term_then_stage_then_generation` (reader, `tests/commit_push.rs`).
