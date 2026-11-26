# Verified: snapshot-partial

- AC-1: `a_partial_index_is_captured_and_failed_shards_together`, `failed_shards_are_written_only_when_there_are_some_and_read_back`.
- AC-2: `failed_shards_that_do_not_cover_exactly_are_corrupt`, `failed_shards_may_come_before_the_captured_ones`, `failed_shards_are_read_and_must_complete_the_index_exactly`.
- AC-3: `the_failed_list_is_bounded_before_it_grows`.
- AC-4: `a_partial_snapshot_says_which_shards_failed`, `the_failures_listed_stop_at_a_thousand_and_the_count_stays_exact`.
- AC-4b: `a_restore_of_an_index_with_failed_shards_is_refused_and_writes_nothing`.

None of these tests was seen red first: they were written with the code.
