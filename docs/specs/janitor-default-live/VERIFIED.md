# Verified

Emulator, so any request number is provisional.

- AC-1: `the_janitor_deletes_by_default` in crates/janitor/src/settings.rs; `a_node_starts_with_its_janitor_live` and `a_janitor_set_to_a_dry_run_counts_and_deletes_nothing` in crates/node/tests/server/assemble.rs; `the_janitor_deletes_unless_the_variable_makes_it_a_dry_run` in crates/node/src/boot/config/tests.rs
- AC-2: `a_writer_that_publishes_again_and_again_loses_only_what_no_commit_names`, `a_successor_after_a_takeover_loses_only_what_no_commit_names` in crates/node/tests/janitor_live_evidence.rs
- AC-3: the tests of docs/specs/janitor-snapshot-pins/VERIFIED.md, docs/specs/janitor-point-in-time-pins/VERIFIED.md and `a_live_janitor_sweep_after_the_take_deletes_no_blob_the_commit_names` in crates/node/tests/mixed_takeover.rs
- AC-4: `a_take_that_fails_leaves_the_shard_to_the_next_write_with_nothing_lost` in crates/node/tests/proactive_faults.rs and `a_reader_sees_the_same_documents_after_a_pass_as_after_a_write_takes_the_shard` in crates/node/tests/proactive_evidence.rs, run in CI under the live default

Not covered: a mixed fleet running live (the Java side is a recording), and a real bucket (emulator only). The revert is `OSS_JANITOR_DRY_RUN=true`.
