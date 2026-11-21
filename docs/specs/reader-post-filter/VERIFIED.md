# Verified: reader-post-filter

- AC-1: `post_filter_narrows_the_hits_and_total_without_changing_scores` in crates/reader/src/search_tests.rs
- AC-2: `post_filter_leaves_the_aggregations_to_the_whole_query` in crates/reader/src/search_tests.rs
- AC-3: `post_filter_parses_one_query_and_is_refused_where_it_cannot_be_served` in crates/reader/src/search_tests.rs
