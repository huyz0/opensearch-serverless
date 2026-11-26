# Reader: nested sorts: verified

Run in the cloud sandbox with `cargo test -p oss-reader --lib`; the tests were seen failing first.

- AC-1: `dsl::sort::tests::a_nested_sort_names_its_path_and_filter_in_either_form`, `a_malformed_nested_sort_fails_as_opensearch_does`.
- AC-2: `dsl::sort::tests::what_a_nested_sort_does_not_serve_is_refused_by_name` and `search_sort::nested_tests::search_after_with_a_nested_sort_is_refused`.
- AC-3: `search_sort::nested_tests::parents_come_by_their_greatest_child_descending_and_their_least_ascending`.
- AC-4: `search_sort::nested_tests::a_filter_chooses_the_children_that_count_and_a_parent_without_one_is_last`.
- AC-5: `search_sort::nested_tests::parents_come_by_their_greatest_child_descending_and_their_least_ascending` (asserts the reported child values).
- AC-6: `search_sort::nested_tests::a_path_that_is_not_a_nested_object_fails_in_words`.
- AC-7: NOT-RUN: not counted by a test; no nested row in the request-budget table yet.
