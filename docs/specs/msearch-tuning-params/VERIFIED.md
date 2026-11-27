# Verified: tuning parameters and `rest_total_hits_as_int` on `_msearch`

- AC-1: `msearch_header_keys_that_tune_a_search_are_accepted_without_effect` in crates/node/tests/it/search_url_params.rs (seen red as a refused header key, green after)
- AC-2: `msearch_header_keys_that_tune_a_search_are_accepted_without_effect` in crates/node/tests/it/search_url_params.rs
- AC-3: `msearch_url_parameters_that_tune_are_accepted_and_the_total_can_be_a_number` in crates/node/tests/it/search_url_params.rs (seen red as an unrecognized parameter, green after)
- AC-4: `msearch_url_parameters_that_tune_are_accepted_and_the_total_can_be_a_number` in crates/node/tests/it/search_url_params.rs
