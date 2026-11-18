# Reader push handler: verification

- AC-1: `a_push_moves_the_reader_onto_the_new_commit_with_one_get` (node, `tests/push_read.rs`).
- AC-2: `a_push_moves_the_reader_onto_the_new_commit_with_one_get` (node, `tests/push_read.rs`, first push before any open) and `the_handler_answers_whether_the_shard_is_open_and_moves_the_reader` (node, `tests/push_read.rs`).
- AC-3: `a_push_that_changes_nothing_leaves_the_index_open` (node, `tests/push_read.rs`).
- AC-4: `the_handler_answers_whether_the_shard_is_open_and_moves_the_reader` (node, `tests/push_read.rs`).
- AC-5: `only_a_newer_push_replaces_the_one_held` and `a_commit_is_newer_by_term_then_stage_then_generation` (reader, `tests/commit_push.rs`).
- AC-6: `a_released_index_is_peeked_no_more_and_opens_afresh` and `peeking_does_not_count_as_a_search` (node, `src/open_indexes.rs`).
- AC-5 also: `a_commit_that_names_no_segments_file_has_no_generation_to_be_newer_by` (reader, `tests/commit_push.rs`).
