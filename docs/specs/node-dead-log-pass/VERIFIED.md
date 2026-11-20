# Verified: node-dead-log-pass

- AC-1: `a_node_that_is_not_a_reaper_makes_no_request` in crates/node/tests/dead_log_pass.rs
- AC-2: `a_reaper_waits_out_a_log_that_is_still_needed_then_collects_it` in crates/node/tests/dead_log_pass.rs
- AC-3: `a_reaper_waits_out_a_log_that_is_still_needed_then_collects_it` in crates/node/tests/dead_log_pass.rs
- AC-4: `the_mailbox_hands_over_the_latest_snapshot_once` in crates/node/tests/dead_log_pass.rs
