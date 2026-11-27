# Parent bit sets are reserved from the memory budget: verified

Run on 2026-11-26 (Linux cloud container, emulator only). The tests were written before the module they call and did not compile until it existed; they were not seen failing on a wrong figure. Nine mutations of the new module (dropping the inner_hits count, the post_filter, must_not clauses, sub-aggregations, the segment slack, the rounding up, a sort's filter, a knn filter and a function entry's filter) each failed a test when applied by hand.

- AC-1: `a_search_with_no_nested_join_reserves_nothing_for_one` (crates/reader/src/search_nested_bytes_tests.rs)
- AC-2: `each_nested_query_node_adds_one_bit_set`, `inner_hits_add_a_bit_set_of_their_own`
- AC-3: `nested_aggregations_and_nested_sorts_each_add_one`
- AC-4: `a_search_reserves_them_with_its_hits` (the figure is `max_doc / 8` rounded up and a word a segment, 19 bytes for the test index)
- AC-5: `a_nested_aggregation_reserves_the_engines_document_lists`, `a_search_reserves_the_lists_with_its_hits` (figure `4 * 18` for the test index). Seen red against a figure that returned 0.

NOT-RUN: a measurement of the engine's real allocation for a parent bit set; a search refused by a small pool because of its nested joins, through a node (the reader-level refusal is `a_pool_that_holds_the_plain_search_refuses_the_one_with_a_nested_aggregation`).
