# Verified

Emulator, so any request number is provisional.

- AC-1: `the_janitor_deletes_by_default` in crates/janitor/src/settings.rs; `a_node_starts_with_its_janitor_live` and `a_janitor_set_to_a_dry_run_counts_and_deletes_nothing` in crates/node/tests/server/assemble.rs; `the_janitor_deletes_unless_the_variable_makes_it_a_dry_run` in crates/node/src/boot/config/tests.rs
- AC-2: `a_writer_that_publishes_again_and_again_loses_only_what_no_commit_names`, `a_successor_after_a_takeover_loses_only_what_no_commit_names` in crates/node/tests/janitor_live_evidence.rs
- AC-3: the pin tests of docs/specs/janitor-snapshot-pins/VERIFIED.md and docs/specs/janitor-point-in-time-pins/VERIFIED.md (the node's pass reads both, built in crates/node/src/assemble.rs), and `a_live_janitor_sweep_after_the_take_deletes_no_blob_the_commit_names` in crates/node/tests/mixed_takeover.rs
- AC-4: `a_live_sweep_after_a_failed_take_and_after_a_lost_answer_deletes_nothing_the_commit_names` in crates/node/tests/proactive_faults.rs
- AC-5: `a_shard_that_slept_and_woke_loses_only_what_no_commit_names` in crates/node/tests/janitor_live_evidence.rs
- AC-6: `a_sweep_between_a_write_and_its_publish_loses_nothing_the_commit_will_name` in the same file
- AC-7: `a_dead_nodes_log_is_collected_once_a_successor_has_published_all_of_it` in crates/node/tests/dead_logs.rs

Not covered: a mixed fleet running live (the Java side is a recording), and a real bucket (emulator only). The revert is `OSS_JANITOR_DRY_RUN=true`.
