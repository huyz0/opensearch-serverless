# Verified: takeover-scale

Emulator, release build, GET/PUT/LIST latency 20 ms plus up to 7 ms (mean 23.5 ms, the Java soak's 20 ms with 1 request in 50 at 200 ms); 3,000 documents over 1,000 shards, 938 shards named; provisional.

- AC-1: `a_dead_nodes_thousand_shards_are_taken_within_a_tenth_of_the_java_time` in crates/node/tests/takeover_scale.rs, run by hand: every named shard taken, no LIST, 17.6 to 17.7 requests a shard.
- AC-2: `a_dead_nodes_thousand_shards_are_taken_within_a_tenth_of_the_java_time` in crates/node/tests/takeover_scale.rs: each shard's highest sequence number on the new node equals the dead node's.
- AC-3: `a_dead_nodes_thousand_shards_are_taken_within_a_tenth_of_the_java_time` in crates/node/tests/takeover_scale.rs: 16 at once 55.5 s against the Java's 57.0 s (limit 62.7 s). At 4 at once, the default then: 128.6 s, 2.3 times the Java. Before the lock change: 411.4 s at 4 and 411.3 s at 16.
- AC-4: `opening_different_shards_at_once_takes_the_time_of_opening_one` in crates/node/tests/takeover_scale.rs: seven shards at once 287 ms, one 287 ms; seen red first (1.99 s with the role-wide lock, 1.09 s with only that lock replaced). The cap still holds: `opens_at_once_under_a_cap_open_the_cap_and_refuse_the_rest` in crates/node/tests/writer_shard_cap.rs, seen failing six runs in six with the cap's open lock removed.

Not covered: RustFS (this run is the emulator), the Java's tail latency, a node under client load while a pass runs at 16 (the 4-wide figure is in proactive-takeover-default), and a shard cap set during a wide pass.
