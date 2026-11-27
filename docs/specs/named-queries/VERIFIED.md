# Verified: named queries

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Expectations are derived from the Java named in the spec, with no recording from stock OpenSearch except the forwarded-shard rows of AC-9, recorded from stock 3.10.0 (the two shell classes compiled against its lib jars). Coverage and mutants were not run in the sandbox.

- AC-1: `match_all_and_its_boost` and `bool_errors_are_object_parsers` in crates/reader/src/dsl/query_tests.rs, `exists_reads_its_field_and_refuses_the_rest` in crates/reader/src/dsl/queries/parse_tests.rs, and `a_name_that_is_not_a_string_and_too_many_names_are_refused` in crates/node/tests/it/search_named_queries.rs
- AC-1 (second slice): `every_query_kind_of_this_slice_takes_a_name` and `a_name_that_is_not_a_string_is_refused_and_null_is_none` in crates/reader/src/dsl/queries/named_tests.rs, and `the_compound_and_full_text_queries_are_named_too` in crates/node/tests/it/search_named_queries.rs
- AC-1 (third slice): `every_query_kind_of_this_slice_takes_a_name` in crates/reader/src/dsl/queries/named_tests.rs (knn), `painless_is_served_only_while_the_switch_is_on` in crates/reader/src/dsl/queries/script_switch_tests.rs (script)
- AC-2: `a_hit_lists_the_named_queries_that_match_it` in crates/node/tests/it/search_named_queries.rs
- AC-3: `a_hit_lists_the_named_queries_that_match_it` in crates/node/tests/it/search_named_queries.rs
- AC-4: `include_named_queries_score_writes_an_object_of_scores` in crates/node/tests/it/search_named_queries.rs (URL parameter; the body key changes nothing)
- AC-5: `a_name_that_is_not_a_string_and_too_many_names_are_refused` in crates/node/tests/it/search_named_queries.rs
- AC-6: `a_name_used_twice_keeps_its_first_query_and_no_name_is_none` in crates/reader/src/search_named_tests.rs (a search with no name collects none and returns before any work)
- AC-7: `naming_a_query_changes_neither_hits_nor_scores` in crates/node/tests/it/search_named_queries.rs
- AC-8: `names_in_the_post_filter_and_the_rescore_count_too` in crates/node/tests/it/search_named_queries.rs
- AC-9: `a_hit_naming_its_matched_queries_is_what_the_java_shell_writes_byte_for_byte` in crates/transport/tests/search_shard.rs (rows recorded from stock 3.10.0 in fixtures/search-shard/java-3.10.0.txt), `a_hit_that_matched_a_named_query_is_answered_in_the_java_layout` in crates/node/src/search_shard_action/tests.rs, `a_row_carries_the_names_its_hit_matched_with_scores_only_when_asked` in crates/reader/src/response_selected_tests.rs, `a_named_query_reaches_the_source_with_its_name` in crates/node/src/search_shard_action/convert_tests.rs
- AC-10: `names_in_an_aggregation_filter_are_listed_on_the_hits_they_match` in crates/node/tests/it/search_named_queries.rs (seen red with the collection removed)
- AC-11: `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did` in crates/compat/tests/node_cluster.rs over fixtures/compat/nested-search/nested-named.json, recorded from stock 3.10.0 (release tarball) and replayed whole

The engine call is covered in lucene-rust by `scores_at_agrees_with_a_whole_leaf_scan_and_visits_only_the_asked_documents` in crates/lucene-search/tests/rescore_fixtures.rs.

Not run: a request-count test for AC-6 (the check reads documents of the hit page only), and a recording of a stock node.
