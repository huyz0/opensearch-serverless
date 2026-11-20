# Verified: node-takeover

- AC-1: `a_successor_restores_the_commit_replays_the_log_and_carries_on` in crates/node/tests/writer_takeover.rs
- AC-2: `a_successor_of_a_shard_everything_of_which_is_published_publishes_nothing` in crates/node/tests/writer_takeover.rs
- AC-3: `a_shard_whose_predecessor_log_cannot_be_read_is_not_held_and_is_tried_again` in crates/node/tests/writer_takeover.rs
- AC-4: `a_shard_with_a_history_is_refused_as_takeover` in crates/node/tests/writer_opener.rs
