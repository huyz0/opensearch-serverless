# Verified: `_msearch`

- AC-1: `each_item_is_its_searchs_response_with_a_status_and_took_follows` in crates/node/tests/it/msearch.rs
- AC-2: `the_path_index_is_the_default_and_a_header_index_overrides_it` in crates/node/tests/it/msearch.rs
- AC-3: `a_failing_item_answers_its_own_error_and_the_others_are_unaffected` in crates/node/tests/it/msearch.rs
- AC-4: `unsupported_headers_answer_a_refused_item` in crates/node/tests/it/msearch.rs
- AC-5: `an_empty_body_a_missing_body_line_and_a_missing_newline_are_refused` in crates/node/tests/it/msearch.rs
- AC-6: `an_item_over_two_shards_is_the_coordinators_response` in crates/node/tests/it/msearch.rs
- AC-7: `an_unknown_parameter_is_refused_and_a_warm_run_makes_no_request` in crates/node/tests/it/msearch.rs
