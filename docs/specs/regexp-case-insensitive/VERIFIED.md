# Verified: regexp with case_insensitive

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Expectations are derived from the Java named in the spec, with no recording from stock OpenSearch. Coverage and mutants were not run in the sandbox.

- AC-1: `a_term_prefix_wildcard_or_regexp_ignores_case_when_asked` in crates/node/tests/it/search_case_insensitive.rs
- AC-2: `a_term_prefix_wildcard_or_regexp_ignores_case_when_asked` in crates/node/tests/it/search_case_insensitive.rs
- AC-3: `what_stays_refused_is_refused` in crates/node/tests/it/search_case_insensitive.rs

The query is read as a case-insensitive regexp by `case_insensitive_term_level_queries_are_read` in crates/reader/src/dsl/queries/parse_tests.rs.

Not run: a recording of a stock node; the error text of an unparseable pattern follows the engine.
