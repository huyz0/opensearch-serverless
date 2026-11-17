# Reader: highlight: verified

Run with `cargo test -p oss-reader -p oss-compat` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `the_defaults_are_those_of_opensearch`, `a_field_option_beats_the_highlights_which_beats_the_default`
- AC-3: `unknown_keys_fail_as_opensearch_fails_them`, `what_is_not_served_yet_is_refused`
- AC-4: `a_plan_names_the_mapped_text_and_keyword_fields`, `a_field_named_twice_keeps_its_last_options`, `a_named_field_of_another_type_is_refused`
- AC-5: `a_match_is_marked_in_its_fragment`, `no_match_size_shows_the_start_of_an_unmatched_field`, `the_whole_value_a_keyword_and_a_html_encoding`, `values_of_an_array_a_path_and_numbers_are_read_as_java_prints_them`, `fields_come_out_in_hash_order_and_blank_fragments_are_dropped`, `score_order_puts_the_best_fragment_first`, `the_word_boundary_and_sentence_scanners_cut_differently`, `the_scratch_bound_grows_with_the_source_and_the_tags`
- AC-6: `a_source_that_cannot_be_highlighted_fails`
