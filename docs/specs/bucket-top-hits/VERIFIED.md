# Verified: top_hits inside a bucket aggregation

- AC-1: `each_bucket_holds_its_own_best_hits_in_the_sort`, `from_keeps_the_hits_ahead_for_a_merge`, `a_top_hits_by_score_holds_scored_hits_in_each_bucket`, `the_shards_response_writes_each_buckets_hits` in crates/reader/src/aggs/bucket/top_hits_sub_tests.rs
- AC-2: `a_histogram_a_range_and_a_filters_hold_each_buckets_best_hit` in crates/reader/src/aggs/bucket/top_hits_sub_tests.rs
- AC-3: `a_bucket_two_shards_hold_has_its_hits_merged_and_written_by_their_shards` in crates/coordinator/tests/it/search_top_hits.rs, `a_top_hits_under_a_bucket_is_merged_across_the_shards` in crates/node/tests/it/coordinate.rs
- AC-4: `a_bucket_one_shard_holds_is_written_from_that_shard` in crates/coordinator/tests/it/search_top_hits.rs
- AC-5: `the_hits_a_bucket_holds_are_reserved_before_the_shard_collects_them` in crates/reader/src/aggs/bucket/top_hits_sub_tests.rs
