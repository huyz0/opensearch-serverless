# Verified

- AC-1: `a_restore_creates_the_index_and_copies_every_file`, `a_restored_index_answers_a_search_with_what_the_snapshot_captured`
- AC-2: `a_target_that_exists_is_refused_before_anything_is_written`, `a_snapshot_that_does_not_exist_is_404`, `an_index_the_snapshot_did_not_capture_is_404`, `a_system_index_is_not_restored`, `nothing_is_restored_into_a_system_index`, `targets_are_resolved_whole_before_anything_is_written`, `what_the_shell_refuses_is_refused_with_its_reason`; NOT-RUN for the route's 409 in-progress answer and for the 4,096-shard bound: the coordinator tests hold each answer's rendering, and no route test reaches either
- AC-3: `a_restore_that_fails_part_way_removes_what_it_created`, `a_manifest_a_writer_landed_first_is_a_conflict_and_is_left_alone`, `a_source_shorter_than_recorded_fails_and_no_manifest_lands`
- AC-4: NOT-RUN: nothing measures peak memory against a large file or the 429 of a failed reservation; the copy streams through two fixed buffers by construction (`an_index_is_restored_by_copying_each_file_and_landing_the_manifest` counts one ranged read for each buffer)
- AC-5: `a_restore_costs_what_the_budget_says`, `a_restore_that_fails_part_way_removes_what_it_created`
- AC-6: `a_restore_with_the_switch_off_answers_501_and_asks_the_store_nothing`, `a_snapshot_that_copied_its_data_is_refused_501`, `what_cannot_be_restored_is_refused_before_anything_is_written`
- AC-7: `a_snapshot_a_java_node_took_is_restored_by_a_rust_node`, `a_restore_writes_the_descriptor_and_manifests_a_java_node_reads`, `fixtures/snapshots/CheckRestore.java` (PASS, run by hand against the shell at commit e71d4233)
- AC-8: `what_cannot_be_restored_is_refused_before_anything_is_written`, `what_a_restore_refuses_before_writing_is_named`; the rollup clause does not apply: this node keeps no rollup or pruning state
