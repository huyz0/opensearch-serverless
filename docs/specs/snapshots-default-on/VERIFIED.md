# Verified

- AC-1: `a_node_with_no_variable_serves_the_snapshot_reads_the_take_and_the_restore` (crates/node/tests/server/boot_snapshots.rs): PASS.
- AC-2: `the_variable_set_to_false_takes_the_snapshot_routes_away` (crates/node/tests/server/boot_snapshots.rs): PASS.
- AC-3: `the_reads_the_take_and_the_restore_are_on_until_set_and_each_stands_alone` (crates/node/src/boot/config/snapshots.rs): PASS.
- AC-4: `a_delete_that_reads_no_snapshots_costs_one_list_more_than_one_that_does_not` (crates/node/tests/server/index_delete.rs): PASS; 11 against 10, over the emulator.
- AC-5: `a_snapshot_taken_beside_a_java_writer_survives_the_index_and_restores_on_java` (crates/node/tests/mixed_fleet/snapshots.rs): PASS, run by hand three times with the take on and three more with the Rust restore switched on, then once with the other three mixed-fleet tests (4 passed, `--test-threads=1`); RustFS 1.0.6 and the shell at its serverlessshell branch.
- AC-6: `an_abort_releases_the_name_and_removes_the_marker`, `an_abort_never_touches_a_later_take_of_the_same_name`, `an_abort_after_the_snapshot_finished_leaves_it_standing`, `a_take_that_was_overtaken_cannot_publish_or_finish` (crates/meta/tests/descriptors/snapshot_take.rs): PASS (10 meta tests and 10 node take tests).
