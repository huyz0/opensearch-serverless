# Verified: named queries

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Expectations are derived from the Java named in the spec, with no recording from stock OpenSearch. Coverage and mutants were not run in the sandbox.

- AC-1: `match_all_and_its_boost` and `bool_errors_are_object_parsers` in crates/reader/src/dsl/query_tests.rs, `exists_reads_its_field_and_refuses_the_rest` in crates/reader/src/dsl/queries/parse_tests.rs, and `a_name_that_is_not_a_string_and_too_many_names_are_refused` in crates/node/tests/it/search_named_queries.rs
- AC-1 (second slice): `every_query_kind_of_this_slice_takes_a_name` and `a_name_that_is_not_a_string_is_refused_and_null_is_none` in crates/reader/src/dsl/queries/named_tests.rs, and `the_compound_and_full_text_queries_are_named_too` in crates/node/tests/it/search_named_queries.rs
- AC-2: `a_hit_lists_the_named_queries_that_match_it` in crates/node/tests/it/search_named_queries.rs
- AC-3: `a_hit_lists_the_named_queries_that_match_it` in crates/node/tests/it/search_named_queries.rs
- AC-4: `include_named_queries_score_writes_an_object_of_scores` in crates/node/tests/it/search_named_queries.rs
- AC-5: `a_name_that_is_not_a_string_and_too_many_names_are_refused` in crates/node/tests/it/search_named_queries.rs
- AC-6: `a_name_used_twice_keeps_its_first_query_and_no_name_is_none` in crates/reader/src/search_named_tests.rs (a search with no name collects none and returns before any work)
- AC-7: `naming_a_query_changes_neither_hits_nor_scores` in crates/node/tests/it/search_named_queries.rs

The engine call is covered in lucene-rust by `scores_at_agrees_with_a_whole_leaf_scan_and_visits_only_the_asked_documents` in crates/lucene-search/tests/rescore_fixtures.rs.

Not run: a request-count test for AC-6 (the check reads documents of the hit page only), and a recording of a stock node.
