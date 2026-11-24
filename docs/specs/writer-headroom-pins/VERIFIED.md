# Verified: headroom below the cap and pinned indexes

- AC-1: `headroom_lets_go_of_more_than_the_one_shard_a_burst_needs` in crates/node/tests/writer_shard_cap.rs
- AC-2: `with_no_headroom_one_shard_goes_per_open` in crates/node/tests/writer_shard_cap.rs
- AC-3: `headroom_stops_at_the_shards_that_can_go` in crates/node/tests/writer_shard_cap.rs
- AC-4: `a_pinned_shard_is_not_let_go_of_even_when_idlest` in crates/node/tests/writer_shard_cap.rs
- AC-5: `a_node_whose_idle_shards_are_all_pinned_refuses_the_open` in crates/node/tests/writer_shard_cap.rs
- AC-6: `a_pinned_shard_does_not_hibernate` in crates/node/tests/writer_shard_cap.rs
- AC-7: `headroom_and_pins_are_off_by_default_and_set_by_their_variables` in crates/node/src/boot/config/tests.rs
