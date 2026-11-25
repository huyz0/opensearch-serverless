# Verified: takeover-concurrency-default

Emulator, release build, 20 ms plus up to 7 ms a request (mean 23.5 ms), provisional.

- AC-1: `proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read` in crates/node/src/boot/config/tests.rs, with the new defaults (16, 60,000); seen failing with the old max_concurrent default.
- AC-2: `one_pass_at_the_shipped_settings_takes_every_shard` in crates/node/tests/takeover_scale.rs, run by hand: 55.4 s, 938 of 938 shards in one pass (strict run after the last edits), 17.6 requests a shard, no LIST (limit 62.7 s). At 30 s (an earlier hand run of a test since replaced by this one) the pass took 512 and deferred 426. 55.5 s against the 60 s limit is thin: at a real network's tail a pass defers its remainder to the retry or the next write.
- AC-3: `write_latency_of_a_live_node_while_a_pass_runs_wide` in crates/node/tests/takeover_scale.rs, run by hand, 3 runs of 200 writes each, 64 shards named, the writes made through the node that runs the pass to a shard of its own: p99 no pass 88.5 ms, 4 wide 88.6 ms, 16 wide 88.6 ms (ratio 1.001); p50 75.4, 75.8 and 76.1 ms. The emulator adds no contention of its own, so this shows what the pass costs the node in CPU and locks on this host, not a real network or a small host.

Not covered: a node with a small memory pool under a 16-wide pass (the deferral path is tested at 4), RustFS, a real network.
