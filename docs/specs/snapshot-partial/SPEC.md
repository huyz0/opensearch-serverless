# Snapshot: a partial snapshot records the shards it could not capture

## Serves

[ADR 0066](../../adr/0066-a-partial-snapshot-records-the-shards-it-could-not-capture.md). This spec is built in two changes: the record and the read side first (AC-1 to AC-4), then the take and its setting (AC-5 to AC-8), then the restore (AC-9 onward).

## Acceptance criteria

- AC-1: An index entry may carry `failed_shards`, ascending shard numbers each below `number_of_shards`. The writer writes it only when not empty. Tests: `a_partial_index_is_captured_and_failed_shards_together`, `failed_shards_are_written_only_when_there_are_some_and_read_back`.
- AC-2: A finished index's captured and failed shards cover every shard exactly once; a gap, a repeat, a number past the count, a shard both captured and failed, or a malformed list is a corrupt record in the summary and the restore reader. Tests: `failed_shards_that_do_not_cover_exactly_are_corrupt`, `failed_shards_may_come_before_the_captured_ones`, `failed_shards_are_read_and_must_complete_the_index_exactly`.
- AC-3: An index with neither captured nor failed shards is the in-progress marker; one whose shards all failed is not. The failed list is bounded by `MAX_SHARDS` before it grows. Tests: `a_partial_index_is_captured_and_failed_shards_together`, `the_failed_list_is_bounded_before_it_grows`.
- AC-4: `GET _snapshot/{repo}/{name}` answers `state` `PARTIAL`, exact `shards.failed`, and at most 1,000 `failures` entries in stock's shape. Tests: `a_partial_snapshot_says_which_shards_failed`, `the_failures_listed_stop_at_a_thousand_and_the_count_stays_exact`.
- AC-4b: Until the restore of a partial snapshot is built, a restore of an index with failed shards is 500 `snapshot_restore_exception` and writes nothing. Test: `a_restore_of_an_index_with_failed_shards_is_refused_and_writes_nothing`.

- AC-5: With `serverless.snapshots.take.partial_shards` on and `partial=true`, an index with a shard that published nothing is kept, the shard is recorded in `failed_shards` and the snapshot answers `PARTIAL`. Test: `a_partial_take_keeps_the_index_and_names_the_shard_that_published_nothing`.
- AC-6: Without `partial=true`, or with the setting off, a take is as before: 409 `nothing_published`, or the index is skipped. Test: `without_partial_or_the_setting_a_shard_that_published_nothing_is_as_before`.
- AC-7: A take in which no shard was captured is 409 `nothing_published`; an index whose every shard failed is kept only beside one that captured a shard. Test: `an_all_failed_index_needs_an_index_that_captured_a_shard`.
- AC-8: The setting defaults to off (`OSS_SNAPSHOTS_TAKE_PARTIAL_SHARDS`). Test: `a_take_keeps_no_failed_shards_until_set`.
- AC-8b: A partial take of an index of three shards, two without a commit, makes 13 requests. Test: `a_partial_take_costs_what_the_budget_holds`.

## S3 request budget

`snapshot.take_partial.requests` = 13: the one-shard take (11) plus one manifest GET for each further shard; the take no longer stops at the first shard with no commit. Everything else is unchanged with the setting off.

## Risks

A Java node that has not been changed refuses a record with failed shards loudly; nothing writes one until the take setting (a later change) is on, and that stays off until every reader has both changes.
