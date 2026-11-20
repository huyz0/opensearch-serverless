# Verified: the janitor's pass over a writer's shards

- AC-1: `a_held_shard_that_published_nothing_makes_no_request` in crates/node/tests/writer_janitor.rs
- AC-2: `an_unreferenced_blob_goes_on_the_second_sighting_and_then_the_shard_rests` in crates/node/tests/writer_janitor.rs
- AC-3: `a_dry_run_counts_and_deletes_nothing` in crates/node/tests/writer_janitor.rs
- AC-4: `a_point_in_time_keeps_the_blob_it_names` in crates/node/tests/writer_janitor.rs
- AC-5: `a_new_commit_makes_the_shard_due_again` in crates/node/tests/writer_janitor.rs
- AC-6: `a_pass_with_no_memory_for_its_buffer_sweeps_nothing` in crates/node/tests/writer_janitor.rs
- AC-7: `a_shard_pinned_whole_is_swept_again_once_the_pin_is_gone` in crates/node/tests/writer_janitor.rs
