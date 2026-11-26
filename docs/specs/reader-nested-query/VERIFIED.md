# Reader: nested query: verified

Run with `cargo test -p oss-reader --lib` on 2026-11-26 (Linux cloud container). The `nested` tests were seen failing before the non-nested filter was in.

- AC-1: `a_nested_query_holds_its_path_query_and_options`, `the_score_mode_defaults_to_avg_and_names_each_mode`, `a_nested_query_without_a_path_or_query_is_refused`
- AC-2: `what_is_not_served_is_refused_by_name`
- AC-3: `nested_objects_are_paths_that_hold_their_fields`
- AC-4: `a_nested_query_matches_the_parents_of_matching_children`
- AC-5: `a_nested_query_scores_its_parents_by_the_score_mode`
- AC-6: `a_nested_query_composes_with_the_queries_around_it`
- AC-7: `an_unmapped_path_fails_unless_it_is_ignored`
- AC-8: `a_search_of_an_index_with_nested_objects_sees_only_the_parents`, `an_index_without_nested_objects_is_searched_as_before`
- AC-9: `explain_and_a_filters_aggregation_see_only_parents`
