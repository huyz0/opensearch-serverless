# Snapshot record: a finished index names every shard exactly once

## Serves

[ADR 0066](../../adr/0066-a-partial-snapshot-records-the-shards-it-could-not-capture.md) lets a record list failed shards, so a reader can no longer take a short `shards` list to mean a whole index. Before any reader learns `failed_shards`, both record readers must refuse a finished index whose captured shards are not exactly its shard slots, so an older node never restores the survivors of a partial snapshot silently.

## Acceptance criteria

- AC-1: The summary parser refuses a finished index (a non-empty `shards`) whose entries are not numbered `0..number_of_shards` each once: a missing, repeated or out-of-range shard, or an entry with no shard number. Tests: `a_finished_index_names_every_shard_exactly_once`, `a_shard_without_its_number_is_refused`.
- AC-2: An index with a shard count and no `shards` stays the in-progress marker. Test: `an_index_with_no_shards_listed_is_still_the_marker`.
- AC-3: The check uses a fixed bitset of `MAX_SHARDS` (4096) slots, so it allocates nothing by the list. An index of 4096 shards is checked; one of more is left to the route, which refuses it 400 (`a_restore_over_4096_shards_is_400_and_writes_nothing`). Test: `the_largest_index_is_checked_and_a_larger_one_is_left_to_the_route`.
- AC-4: The restore reader refuses the same cases with a corrupt-record error (500 `snapshot_unreadable`) and bounds the list before pushing. Tests: `a_finished_index_must_name_every_shard_once`, `the_largest_index_is_checked_and_a_larger_one_is_left_to_the_route`.
- AC-5: Every Java-written fixture still reads. Test: the existing `snapshot_java_fixtures` suite.

## S3 request budget

None: parsing only.

## Risks

A record written by any node that lists fewer shards than its count is now refused; none is written today, and the fixtures pin it.
