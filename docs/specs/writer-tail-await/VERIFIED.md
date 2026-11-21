# Verified: writer-tail-await

- AC-1: `a_shard_nothing_is_kept_for_waits_for_nothing` in crates/node/tests/tail_await.rs
- AC-2: `the_wait_pushes_at_once_and_ends_when_every_node_answered` in crates/node/tests/tail_await.rs
- AC-3: `a_lost_push_is_not_reached_and_an_unwanted_answer_is` in crates/node/tests/tail_await.rs
- AC-4: `a_wait_times_out_and_ends_when_the_shard_is_let_go` in crates/node/tests/tail_await.rs
- AC-5: `grants_wait_out_a_shard_this_node_only_just_began_keeping` in crates/node/tests/tail_await.rs
- AC-6: `grants_wait_out_the_grant_of_a_node_no_longer_pushed_to` in crates/node/tests/tail_await.rs
