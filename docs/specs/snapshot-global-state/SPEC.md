# Snapshot: a snapshot captures the global state it is asked for

## Serves

[ADR 0068](../../adr/0068-a-snapshot-captures-the-global-state-it-is-asked-for.md). Built in three changes: the record member first (AC-1 to AC-4), then the take and its setting (AC-5 to AC-9), then the restore (AC-10 to AC-16). Each change adds its criteria here.

## Acceptance criteria

- AC-1: A record may carry one top-level `global_state` member with `persistent_settings` (scalars) and the five document sets; the writer writes it only when not empty, and the summary readers skip it. Tests: `what_is_written_is_read_back_and_an_empty_state_writes_nothing`, `an_unknown_member_is_skipped`.
- AC-2: The record the Java shell wrote (`fixtures/snapshots/record-with-global-state.json`) is read in full. Test: `the_record_the_java_wrote_is_read_in_full`.
- AC-3: A restore-side read is strict: a member that is not an object, a non-scalar or null setting, a repeated name, a document that is not an object, or a set that is not an object is a corrupt record. Test: `a_malformed_state_is_refused`.
- AC-4: The state is at most 4 MiB encoded: exactly 4 MiB is accepted, one byte more is `TooLarge` naming the settings or the set that crossed it, and a record read past it is refused before it is decoded. Tests: `a_state_of_exactly_the_limit_is_taken_and_one_byte_more_names_the_set`, `settings_that_pass_the_limit_name_the_settings`, `a_state_past_the_limit_is_refused_on_read`.

- AC-5: With `serverless.snapshots.take.capture_global_state` on, a take with `include_global_state: true` reads the settings and the five sets before any shard work and records them in the record; the answer and GET say `include_global_state: true`. Test: `a_take_that_asks_records_the_settings_and_every_set`.
- AC-6: Without the flag, or with the setting off, nothing is recorded and the snapshot answers `include_global_state: false` (stock says true; a **Differs**). The setting defaults to off (`OSS_SNAPSHOTS_TAKE_CAPTURE_GLOBAL_STATE`). Tests: `without_the_flag_or_the_setting_nothing_is_recorded_and_the_answer_says_false`, `a_take_captures_no_global_state_until_set`.
- AC-7: An empty state writes no member. Test: `an_empty_state_writes_no_member`.
- AC-8: A state past 4 MiB is refused 400 `snapshot_too_large` naming the set that crossed it, before any snapshot name is reserved. Test: `a_state_past_the_limit_is_refused_naming_the_set_and_costs_no_shard_read`.
- AC-9: A one-shard take over an empty state makes 17 requests, over a state of one template and one script 19. Test: `a_take_with_state_costs_what_the_budget_holds`.

- AC-10: A restore with `include_global_state: true` puts each document of the snapshot's state back by name through the validator of its PUT route and merges the persistent settings, after every index is whole; it deletes nothing and answers `global_state_restored`. Test: `a_restore_puts_the_documents_back_and_merges_the_settings_without_deleting`.
- AC-11: Without the flag nothing is applied and the answer carries no report. Test: `without_the_flag_the_state_is_left_alone`.
- AC-12: A snapshot with no state answers `global_state_restored: 0`; a restore that selects no index applies only the state. Test: `a_snapshot_without_state_answers_zero_and_no_indices_applies_only_the_state`.
- AC-13: A document that no longer validates is named in `failed_global_state` as `<kind>/<name>`, the rest are applied and the restored indices stay. Tests: `a_document_that_no_longer_validates_is_named_and_the_rest_are_applied`, `each_set_applies_the_validator_of_its_put_route`.
- AC-14: A restore after which a set would hold more than 1,000 documents is 400 `illegal_argument_exception` naming the set, before any write; exactly 1,000 is applied. Tests: `a_set_past_a_thousand_documents_is_refused_and_exactly_a_thousand_is_not`, `the_refusal_is_a_400_naming_the_set`.
- AC-15: A background restore applies the state once its indices are whole; a cancelled step counts what it did not apply and applies none. Tests: `a_background_restore_applies_the_state_after_the_indices`, `a_cancelled_global_state_step_names_the_count_and_applies_nothing`.
- AC-16: A restore selecting no index from a snapshot without state makes 1 request; from a state of one template, one script and two settings 14. Test: `a_restore_of_the_state_costs_what_the_budget_holds`.
- AC-17: The report is written as the answer's members: the count, and the failed names when there are any. Test: `the_answer_names_what_was_applied_and_what_failed`.
- AC-18: A Java node reads the record a Rust take wrote with every part of the state: the shell's own `SnapshotRecord.fromStream`, `SnapshotGlobalState.read`, a `toBytes` round trip that keeps the member, and the bounds check. Tests: `a_take_writes_the_global_state_record_a_java_node_reads` (writes `fixtures/snapshots/rust-take-global/state.bin`), `ServerlessRustGlobalStateTests` (run by hand, CI has no Java).
- AC-19: A document set that changes while the take reads it is read again from its newer marker: a script deleted after the walk began on the contents is not in the record, each document is held once, and the take answers 200. Test: `a_set_that_changes_mid_walk_is_read_again_and_each_document_is_held_once`.
- AC-20: A settings merge that fails is named `persistent_settings` in `failed_global_state`, the answer is 200 and the settings stay as they were. Test: `a_failing_settings_merge_is_named_and_leaves_the_settings`.
- AC-21: A background restore thread that panics after every index is whole keeps the indices and gives back its place and bytes. Test: `a_thread_that_panics_after_the_indices_are_whole_keeps_them_and_frees_its_slot`.
- AC-22: A Java node restores, with `include_global_state`, the snapshot a Rust take wrote with state, and reads back the settings and each of the five sets. Test: `ServerlessRustGlobalRestoreTests` over the bucket `a_restored_index_answers_a_search_with_what_the_snapshot_captured` writes (run by hand, CI has no Java).

Security: every `/_snapshot` route needs `all_access` in this node (`every_repository_and_snapshot_route_needs_all_access`), which is stronger than the action ADR 0068 names, so no second check is made here.

## S3 request budget

`snapshot.restore_global_state_empty.requests` = 1 and `snapshot.restore_global_state_one_doc.requests` = 14 (measured; the pre-check reads one marker for each set that has documents and one for the settings). `snapshot.take_global_state_empty.requests` = 17 and `snapshot.take_global_state_one_doc.requests` = 19: the one-shard take (11), the settings register and the five set markers (6), one content GET for each document. No LIST, nothing scales with indices.

## Risks

Nothing writes the member unless the take setting is on, and that stays off until the Java carry-through has shipped (it has: huyz0/OpenSearch #186).
