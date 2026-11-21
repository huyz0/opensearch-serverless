# Verified: proactive-takeover-default

- AC-1: `proactive_takeover_is_on_with_the_settings_defaults_and_each_is_read` in crates/node/src/boot/config.rs
- AC-2: `a_booted_node_takes_a_dead_nodes_shard_before_any_write_only_when_the_setting_is_on` in crates/node/tests/boot.rs
- AC-3: `with_the_switch_on_a_fleet_with_no_death_makes_no_request` in crates/node/tests/proactive_evidence.rs
- AC-4: NOT-RUN: held by docs/specs/proactive-takeover-faults/VERIFIED.md, each line named there
