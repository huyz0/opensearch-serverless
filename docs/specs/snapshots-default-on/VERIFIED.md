# Verified

- AC-1: `a_node_with_no_variable_serves_the_snapshot_reads_but_neither_take_nor_restore` (crates/node/tests/server/boot_snapshots.rs): PASS.
- AC-2: `the_variable_set_to_false_takes_the_snapshot_routes_away` (crates/node/tests/server/boot_snapshots.rs): PASS.
- AC-3: `the_reads_are_on_and_take_and_restore_off_until_set_and_each_stands_alone` (crates/node/src/boot/config/snapshots.rs): PASS.
- AC-4: `a_delete_that_reads_no_snapshots_costs_one_list_more_than_one_that_does_not` (crates/node/tests/server/index_delete.rs): PASS; 11 against 10, over the emulator.
