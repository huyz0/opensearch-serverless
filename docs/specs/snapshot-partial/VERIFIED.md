# Verified: snapshot-partial

- AC-1: `a_partial_index_is_captured_and_failed_shards_together`, `failed_shards_are_written_only_when_there_are_some_and_read_back`.
- AC-2: `failed_shards_that_do_not_cover_exactly_are_corrupt`, `failed_shards_may_come_before_the_captured_ones`, `failed_shards_are_read_and_must_complete_the_index_exactly`.
- AC-3: `the_failed_list_is_bounded_before_it_grows`.
- AC-4: `a_partial_snapshot_says_which_shards_failed`, `the_failures_listed_stop_at_a_thousand_and_the_count_stays_exact`.
- AC-4b: `a_restore_of_an_index_with_failed_shards_is_refused_and_writes_nothing`.
- AC-5: `a_partial_take_keeps_the_index_and_names_the_shard_that_published_nothing`.
- AC-6: `without_partial_or_the_setting_a_shard_that_published_nothing_is_as_before`.
- AC-7: `an_all_failed_index_needs_an_index_that_captured_a_shard`.
- AC-8: `a_take_keeps_no_failed_shards_until_set`.
- AC-8b: `a_partial_take_costs_what_the_budget_holds`.

None of these tests was seen red first: they were written with the code. NOT-RUN: a Java node reading a Rust-written partial record (the cross-shell fixture), and the Java take writing one that Rust reads.
