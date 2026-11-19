# Verified: the writer role publishes the shards it holds

- AC-1: `the_first_pass_publishes_a_written_shard_for_a_blob_and_a_swap` in crates/node/tests/writer_publish.rs
- AC-2: `a_shard_with_no_write_publishes_nothing` in crates/node/tests/writer_publish.rs
- AC-3: `a_later_write_waits_for_its_age_rounded_up_to_the_tick` in crates/node/tests/writer_publish.rs
- AC-4: `age_zero_publishes_every_pass_with_a_write` in crates/node/tests/writer_publish.rs
- AC-5: `a_refused_publish_is_reported_and_tried_again` in crates/node/tests/writer_publish.rs
- AC-6: `a_reopened_shard_publishes_at_its_first_write` in crates/node/tests/writer_publish.rs
