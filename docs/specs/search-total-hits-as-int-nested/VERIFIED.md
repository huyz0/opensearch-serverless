# Verified: `rest_total_hits_as_int` inside `top_hits` and `inner_hits`

- AC-1: `the_total_inside_a_top_hits_is_a_number_too` in crates/node/tests/it/search_url_params.rs (seen red as an object inside the aggregation, green after)
- AC-2: `the_total_inside_a_top_hits_over_several_indexes_is_a_number_too` in crates/node/tests/it/search_url_params.rs (seen red, green after)
- AC-3: `the_total_of_inner_hits_is_a_number_under_rest_total_hits_as_int` in crates/reader/src/search_inner_hits_tests.rs (seen red as an object, green after), and `a_total_is_an_object_outside_a_scope_and_a_number_inside_one` in crates/reader/src/hits_total.rs
