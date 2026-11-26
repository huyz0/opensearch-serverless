# Snapshot: a partial snapshot records the shards it could not capture

## Serves

[ADR 0066](../../adr/0066-a-partial-snapshot-records-the-shards-it-could-not-capture.md). This spec is built in two changes: the record and the read side first (AC-1 to AC-4), then the take, the setting and the restore (AC-5 onward).

## Acceptance criteria

- AC-1: An index entry may carry `failed_shards`, ascending shard numbers each below `number_of_shards`. The writer writes it only when not empty. Tests: `a_partial_index_is_captured_and_failed_shards_together`, `failed_shards_are_written_only_when_there_are_some_and_read_back`.
- AC-2: A finished index's captured and failed shards cover every shard exactly once; a gap, a repeat, a number past the count, a shard both captured and failed, or a malformed list is a corrupt record in the summary and the restore reader. Tests: `failed_shards_that_do_not_cover_exactly_are_corrupt`, `failed_shards_may_come_before_the_captured_ones`, `failed_shards_are_read_and_must_complete_the_index_exactly`.
- AC-3: An index with neither captured nor failed shards is the in-progress marker; one whose shards all failed is not. The failed list is bounded by `MAX_SHARDS` before it grows. Tests: `a_partial_index_is_captured_and_failed_shards_together`, `the_failed_list_is_bounded_before_it_grows`.
- AC-4: `GET _snapshot/{repo}/{name}` answers `state` `PARTIAL`, exact `shards.failed`, and at most 1,000 `failures` entries in stock's shape. Tests: `a_partial_snapshot_says_which_shards_failed`, `the_failures_listed_stop_at_a_thousand_and_the_count_stays_exact`.
- AC-4b: Until the restore of a partial snapshot is built, a restore of an index with failed shards is 500 `snapshot_restore_exception` and writes nothing. Test: `a_restore_of_an_index_with_failed_shards_is_refused_and_writes_nothing`.

## S3 request budget

None added in this change: reading only.

## Risks

A Java node that has not been changed refuses a record with failed shards loudly; nothing writes one until the take setting (a later change) is on, and that stays off until every reader has both changes.
