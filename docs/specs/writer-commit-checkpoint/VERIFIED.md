# Verified: writer-commit-checkpoint

- AC-1: `a_published_commit_answers_the_highest_sequence_number_it_holds` in crates/writer/tests/commit_checkpoint.rs
- AC-2: `a_shard_with_no_commit_answers_none` in crates/writer/tests/commit_checkpoint.rs
- AC-3: `reading_a_checkpoint_is_a_register_read_and_one_ranged_get_and_no_list_or_put` in crates/writer/tests/commit_checkpoint.rs
