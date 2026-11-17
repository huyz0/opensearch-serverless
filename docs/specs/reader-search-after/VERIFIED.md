# Reader: search_after: verified

Run with `cargo test -p oss-reader -p oss-compat` and `scripts/gate.sh` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `values_are_read_as_jackson_types_them`, `a_bad_search_after_fails_as_opensearch_fails`
- AC-3: `an_int_key_narrows_a_number_and_parses_a_string`, `a_score_key_reads_a_float_as_its_bits`, `a_long_key_parses_a_long_or_a_double_rounded_down`, `a_date_key_parses_a_date_and_refuses_now`, `the_page_needs_from_zero_a_sort_and_a_value_a_key`, `a_key_is_read_from_its_sort_field`
- AC-4: `a_null_for_a_numeric_key_fails_after_every_value_converts`, `a_keyword_key_takes_the_value_s_text`
