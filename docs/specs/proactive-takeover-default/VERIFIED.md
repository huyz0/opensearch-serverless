# Verified: proactive-takeover-default

- AC-1: `proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read` in crates/node/src/boot/config.rs
- AC-2: `a_booted_node_takes_a_dead_nodes_shard_before_any_write_only_when_the_setting_is_on` in crates/node/tests/server/boot.rs
- AC-3: `with_the_switch_on_a_fleet_with_no_death_makes_no_request` in crates/node/tests/proactive_evidence.rs
- AC-4: NOT-RUN: held by docs/specs/proactive-takeover-faults/VERIFIED.md, each line named there
- AC-5: `a_search_after_a_pass_makes_the_requests_of_a_search_after_a_lazy_take` in crates/node/tests/proactive_evidence.rs
- AC-6: `search_latency_after_a_take_and_while_a_pass_runs` in crates/node/tests/proactive_evidence.rs, run by hand: open and a match-all for ten hits, 150 searches each side, emulator GET 20-30 ms, PUT 40-60 ms (provisional): after a take p99 92.8 ms lazy vs 90.9 ms pass (0.980); during a pass over four dead shards p99 89.1 vs 88.8 ms (0.997); p50 within 2%.
- AC-7: `the_takeover_scenario_with_the_pass_off_and_on_over_rustfs` and `the_takeover_scenario_with_the_pass_off_and_on_over_the_emulator` in crates/node/tests/proactive_suite.rs, run by hand (RustFS 1.0.6, zip SHA-256 a4d109a0...1398f, one run each, 3 nodes, 6 shards, 60 writes and 30 searches after the kill): RustFS, pass off vs on: slowest write 371 vs 294 ms, write p99 303 vs 265 ms, search p99 69 vs 57 ms. Emulator at the Java latency profile: slowest write 937 vs 795 ms, write p99 393 vs 404 ms (1.03x), search p99 265 vs 265 ms, requests since the kill 514 vs 538 (+24, +4.7%).

