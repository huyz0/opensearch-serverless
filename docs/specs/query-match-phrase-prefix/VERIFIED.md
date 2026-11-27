# Verified: the `match_phrase_prefix` query

- AC-1: `terms_become_a_multi_phrase_prefix` in crates/reader/src/dsl/queries/phrase_prefix_tests.rs
- AC-2: `other_types_empty_text_and_refusals` in crates/reader/src/dsl/queries/phrase_prefix_tests.rs
- AC-3: `the_last_term_matches_by_prefix_in_a_phrase` in crates/node/tests/it/search_match_phrase_prefix.rs
