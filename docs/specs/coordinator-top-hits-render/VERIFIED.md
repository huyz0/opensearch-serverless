# Verified: the coordinator fetches and renders the hits of a merged top_hits

- AC-1: `the_hits_ahead_of_from_are_explained_only_when_asked` in crates/reader/src/response_selected_tests.rs
- AC-2: `selected_hits_are_written_as_the_response_writes_them`, `a_selection_follows_the_order_asked_and_may_repeat` in crates/reader/src/response_selected_tests.rs
- AC-3: `explain_sort_fields_and_source_filtering_are_written` in crates/reader/src/response_selected_tests.rs
- AC-4: `no_position_or_one_beyond_the_result_is_refused` in crates/reader/src/response_selected_tests.rs
- AC-5: `hits_are_written_in_page_order_with_one_call_a_shard`, `no_score_is_written_as_null` in crates/coordinator/tests/top_hits_render.rs
- AC-6: `a_failing_shard_fails_the_render_and_is_named`, `an_answer_of_the_wrong_length_fails_the_render`, `a_panicking_fetch_fails_the_render_and_names_its_shard` in crates/coordinator/tests/top_hits_render.rs
- AC-7: `an_empty_page_writes_an_empty_list_without_asking_anyone` in crates/coordinator/tests/top_hits_render.rs
