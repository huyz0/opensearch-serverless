# Snapshot: a snapshot captures the global state it is asked for

## Serves

[ADR 0068](../../adr/0068-a-snapshot-captures-the-global-state-it-is-asked-for.md). Built in three changes: the record member first (AC-1 to AC-4), then the take and its setting (AC-5 to AC-9), then the restore. Each change adds its criteria here.

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

## S3 request budget

`snapshot.take_global_state_empty.requests` = 17 and `snapshot.take_global_state_one_doc.requests` = 19: the one-shard take (11), the settings register and the five set markers (6), one content GET for each document. No LIST, nothing scales with indices.

## Risks

Nothing writes the member unless the take setting is on, and that stays off until the Java carry-through has shipped (it has: huyz0/OpenSearch #186).
