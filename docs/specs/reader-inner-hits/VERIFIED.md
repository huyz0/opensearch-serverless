# Reader: nested inner_hits: verified

Run in the cloud sandbox with `cargo test -p oss-reader --lib`.

- AC-1: `dsl::queries::inner_hits_tests::*` (defaults, options, negative window, refusals, unknown key); the window limit is `search_inner_hits_tests::a_window_over_the_limit_and_a_repeated_name_fail_in_words`.
- AC-2: `search_inner_hits_tests::a_parent_carries_the_children_that_matched_under_the_path`.
- AC-3: `search_inner_hits_tests::from_and_size_page_the_children_and_the_total_counts_them_all`.
- AC-4: `search_inner_hits_tests::a_parent_carries_the_children_that_matched_under_the_path` and `fetch::nested_source::tests::the_element_at_the_offset_of_the_array_under_the_path`.
- AC-5: `search_inner_hits_tests::a_nested_query_without_inner_hits_writes_none` and `a_window_over_the_limit_and_a_repeated_name_fail_in_words`.
- AC-6: `inner_hits_and_a_nested_sort_add_no_request_to_a_cold_search` in crates/reader/tests/nested_requests.rs: a cold open and search of a nested index makes the same requests with inner_hits as without (not seen red: it compares two counts, written with the code).
- NOT-RUN: a parent with children of two nested paths (the offset counts children of the path only; the test index has one path), and a multi-shard search through the coordinator.
