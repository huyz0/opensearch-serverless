# Verified: common URL parameters on `_search`

- AC-1: `track_total_hits_sets_the_total_and_beats_the_body` in crates/node/tests/it/search_url_params.rs (seen red as an unrecognized parameter, green after)
- AC-2: `explain_version_and_sequence_numbers_decorate_each_hit` in crates/node/tests/it/search_url_params.rs
- AC-3: `terminate_after_stops_collecting_and_refuses_a_negative` in crates/node/tests/it/search_url_params.rs
- AC-4: `source_parameters_filter_each_hit` in crates/node/tests/it/search_url_params.rs
- AC-5: `sort_orders_the_hits_and_replaces_the_body` in crates/node/tests/it/search_url_params.rs
- AC-6: `tuning_parameters_change_nothing` in crates/node/tests/it/search_url_params.rs
- AC-7: `what_is_not_served_is_still_refused` in crates/node/tests/it/search_url_params.rs
- AC-8: `a_malformed_value_is_refused_as_opensearch_refuses_it` in crates/node/tests/it/search_url_params.rs
