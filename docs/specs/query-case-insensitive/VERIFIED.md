# Verified: `case_insensitive` on `term`, `prefix` and `wildcard`

- AC-1: `a_wildcard_pattern_matches_in_either_case` and `a_built_clause_is_an_automaton_multi_term_query` in crates/reader/src/dsl/queries/case_insensitive_tests.rs
- AC-2: `a_term_prefix_wildcard_or_regexp_ignores_case_when_asked` in crates/node/tests/it/search_case_insensitive.rs and `case_insensitive_term_level_queries_are_read` in crates/reader/src/dsl/queries/parse_tests.rs
- AC-3: `what_stays_refused_is_refused` in crates/node/tests/it/search_case_insensitive.rs
