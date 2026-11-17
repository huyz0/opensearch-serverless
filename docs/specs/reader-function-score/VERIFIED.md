# Reader: function_score: verified

Run with `cargo test -p oss-reader -p oss-compat` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `the_defaults_are_those_of_opensearch`, `a_single_function_is_one_entry`, `every_key_is_read`, `the_modes_and_modifiers_have_their_names`, `a_decay_keeps_its_parameters_for_the_build`, `a_function_error_is_opensearchs`, `an_unknown_function_names_the_registrys_cause`, `a_nested_query_is_parsed_with_its_errors`, `the_boost_is_the_querys`
- AC-3: `no_functions_is_the_query_alone`, `a_replacing_weight_does_not_read_the_query_score`, `the_settings_reach_the_node`, `a_field_value_factor_keeps_its_settings`, `a_field_value_factor_on_an_unmapped_field_needs_missing`, `a_field_value_factor_reads_numbers_and_dates_only`, `a_numeric_decay_is_processed_as_opensearch_processes_it`, `a_date_decay_reads_times`, `a_decay_fails_as_opensearch_fails`, `a_parse_failure_is_wrapped_as_a_query_shard_exception`, `a_decay_needs_a_mapped_numeric_field_and_a_date_an_origin`, `an_opensearch_wrapper_of_an_opensearch_exception_is_unwrapped_at_the_top`
- AC-4: `what_is_not_served_is_refused`
