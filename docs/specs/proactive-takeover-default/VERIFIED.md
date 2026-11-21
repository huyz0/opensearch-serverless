# Verified: proactive-takeover-default

- AC-1: `proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read` in crates/node/src/boot/config.rs
- AC-2: `a_booted_node_takes_a_dead_nodes_shard_before_any_write_only_when_the_setting_is_on` in crates/node/tests/boot.rs
- AC-3: `with_the_switch_on_a_fleet_with_no_death_makes_no_request` in crates/node/tests/proactive_evidence.rs
- AC-4: NOT-RUN: held by docs/specs/proactive-takeover-faults/VERIFIED.md, each line named there
- AC-5: `a_search_after_a_pass_makes_the_requests_of_a_search_after_a_lazy_take` in crates/node/tests/proactive_evidence.rs
- AC-6: `search_latency_after_a_take_and_while_a_pass_runs` in crates/node/tests/proactive_evidence.rs, run by hand: open and a match-all for ten hits, 150 searches each side, emulator GET 20-30 ms, PUT 40-60 ms (provisional): after a take p99 92.8 ms lazy vs 90.9 ms pass (0.980); during a pass over four dead shards p99 89.1 vs 88.8 ms (0.997); p50 within 2%.
