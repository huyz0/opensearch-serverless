# Verified: the coordinator renders cat tables and decides the cat and list index requests

- AC-1: `text_pads_to_the_widest_cell_and_headers_are_optional` and `a_missing_cell_is_null` in crates/coordinator/tests/cat.rs
- AC-2: `structured_is_strings_and_a_page_carries_its_token` and `format_picks_text_or_structured` in crates/coordinator/tests/cat.rs
- AC-3: `h_and_s_are_refused_in_the_shells_words` in crates/coordinator/tests/cat.rs
- AC-4: `the_health_row_names_the_cluster_and_dashes_an_unasked_shard_count` in crates/coordinator/tests/cat.rs
- AC-5: `an_index_row_is_open_with_no_replicas` in crates/coordinator/tests/cat.rs
- AC-6: `a_request_is_refused_in_order_and_walks_or_pages` in crates/coordinator/tests/cat.rs
- AC-7: `a_token_round_trips_and_anything_else_is_tainted` in crates/coordinator/tests/cat.rs
