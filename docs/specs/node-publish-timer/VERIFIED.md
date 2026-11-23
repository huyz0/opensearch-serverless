# Verified: the writer role's publish timer

- AC-1: `a_shard_written_after_the_timer_started_is_published` in crates/node/tests/roles/writer_publish_timer.rs
- AC-2: `dropping_the_timer_stops_it_promptly_and_for_good` in crates/node/tests/roles/writer_publish_timer.rs
- AC-3: `idle_ticks_make_no_request` in crates/node/tests/roles/writer_publish_timer.rs
