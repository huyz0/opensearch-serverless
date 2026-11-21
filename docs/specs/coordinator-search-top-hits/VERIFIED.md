# Verified: a search answers a top-level top_hits across its shards

- AC-1: `the_aggregation_holds_the_merged_page_from_the_shards_that_ranked_it` in crates/coordinator/tests/search_top_hits.rs
- AC-2: `from_is_applied_once_over_all_the_shards` in crates/coordinator/tests/search_top_hits.rs
- AC-3: `a_top_hits_the_request_does_not_describe_is_refused_as_before` in crates/coordinator/tests/search_top_hits.rs
- AC-4: `a_top_hits_is_merged_across_the_shards_and_its_hits_come_from_them` in crates/node/tests/it/coordinate.rs
