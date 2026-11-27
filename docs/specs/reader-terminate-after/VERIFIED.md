# Verified: reader-terminate-after

- AC-1: `terminate_after_keeps_the_first_matches_in_index_order` in crates/reader/src/search_tests.rs
- AC-2: `terminate_after_past_the_matches_collects_them_all_and_does_not_stop` in crates/reader/src/search_tests.rs
- AC-3: `terminated_early_follows_timed_out_when_a_shard_says_and_is_or_of_the_shards` in crates/coordinator/tests/it/search.rs
- AC-4: `terminate_after_is_refused_where_it_cannot_be_served` in crates/reader/src/search_tests.rs
