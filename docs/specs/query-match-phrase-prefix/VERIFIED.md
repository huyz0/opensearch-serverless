# Verified: the `match_phrase_prefix` query

- AC-1: `terms_become_a_multi_phrase_prefix` in crates/reader/src/dsl/queries/phrase_prefix_tests.rs
- AC-2: `other_types_empty_text_and_refusals` in crates/reader/src/dsl/queries/phrase_prefix_tests.rs
- AC-3: `the_last_term_matches_by_prefix_in_a_phrase` in crates/node/tests/it/search_match_phrase_prefix.rs
- AC-4: `the_field_of_a_phrase_prefix_is_a_scored_field` in crates/reader/src/dsl/queries/phrase_prefix_tests.rs; the stock answer replays in `an_assembled_node_answers_every_recorded_gap_as_stock_did` (crates/compat/tests/node_cluster.rs). Before the fix the node scored 1.2327709 where stock gave 0.8474184: the field's norms were not opened, so each document's length read as 1.
