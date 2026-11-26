# Snapshot: a snapshot captures the global state it is asked for

## Serves

[ADR 0068](../../adr/0068-a-snapshot-captures-the-global-state-it-is-asked-for.md). Built in three changes: the record member first (AC-1 to AC-4), then the take and its setting, then the restore and its permission. Each change adds its criteria here.

## Acceptance criteria

- AC-1: A record may carry one top-level `global_state` member with `persistent_settings` (scalars) and the five document sets; the writer writes it only when not empty, and the summary readers skip it. Tests: `what_is_written_is_read_back_and_an_empty_state_writes_nothing`, `an_unknown_member_is_skipped`.
- AC-2: The record the Java shell wrote (`fixtures/snapshots/record-with-global-state.json`) is read in full. Test: `the_record_the_java_wrote_is_read_in_full`.
- AC-3: A restore-side read is strict: a member that is not an object, a non-scalar or null setting, a repeated name, a document that is not an object, or a set that is not an object is a corrupt record. Test: `a_malformed_state_is_refused`.
- AC-4: The state is at most 4 MiB encoded: exactly 4 MiB is accepted, one byte more is `TooLarge` naming the settings or the set that crossed it, and a record read past it is refused before it is decoded. Tests: `a_state_of_exactly_the_limit_is_taken_and_one_byte_more_names_the_set`, `settings_that_pass_the_limit_name_the_settings`, `a_state_past_the_limit_is_refused_on_read`.

## S3 request budget

None in this change: it only encodes and decodes.

## Risks

Nothing writes the member until the take setting (a later change) is on, and that stays off until the Java carry-through has shipped (it has: huyz0/OpenSearch #186).
