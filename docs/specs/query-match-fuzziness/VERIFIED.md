# Verified: `fuzziness` on a `match` query

- AC-1: `a_fuzzy_match_makes_each_analysed_term_a_fuzzy_query` in crates/reader/src/dsl/queries/build_tests.rs and `a_misspelt_term_matches_with_fuzziness` in crates/node/tests/it/search_match_fuzziness.rs
- AC-2: `a_fuzzy_match_keeps_its_fuzzy_options_and_reaches_keyword_fields` in crates/reader/src/dsl/queries/build_tests.rs
- AC-3: `a_fuzzy_match_keeps_its_fuzzy_options_and_reaches_keyword_fields` and `a_fuzzy_match_without_fuzziness_stays_a_plain_match` in crates/reader/src/dsl/queries/build_tests.rs
