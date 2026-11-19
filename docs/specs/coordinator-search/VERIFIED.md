# Verified: the coordinator runs one search over its shards

- AC-1: `the_page_is_the_merged_order_of_the_shards_hits` in crates/coordinator/tests/search.rs
- AC-2: `a_shard_renders_once_and_only_when_the_page_draws_on_it` in crates/coordinator/tests/search.rs
- AC-3: `the_response_envelope_is_written_exactly` in crates/coordinator/tests/search.rs
- AC-4: `an_untracked_total_is_not_written` in crates/coordinator/tests/search.rs
- AC-5: `a_failed_shard_is_counted_and_listed` in crates/coordinator/tests/search.rs
- AC-6: `every_shard_failing_fails_the_search` in crates/coordinator/tests/search.rs
- AC-7: `a_failed_render_fails_the_search` in crates/coordinator/tests/search.rs
- AC-8: `aggregations_are_reduced_and_written` in crates/coordinator/tests/search.rs
