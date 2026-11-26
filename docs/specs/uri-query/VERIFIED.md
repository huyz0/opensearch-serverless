# Verified: `q` URL parameter

- AC-1: `q_searches_as_the_same_query_string_in_the_body` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-2: `q_replaces_a_query_in_the_body_of_a_search` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-3: `the_companions_shape_the_query` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-4: `a_count_with_q_counts_that_query` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-5: `q_counts_and_searches_the_same_over_two_shards` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-6: `a_q_that_cannot_be_parsed_or_a_bad_flag_is_a_400` (oss-node, `crates/node/tests/it/uri_query.rs`)
- AC-7: `quotes_backslashes_and_control_characters_stay_in_the_text`, `a_flag_that_is_not_true_or_false_is_refused` and `an_operator_or_analyzer_the_query_refuses_is_refused` (oss-reader, `crates/reader/src/dsl/uri_query_tests.rs`)
