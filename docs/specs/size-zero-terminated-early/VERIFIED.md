# Verified: size-zero-terminated-early

Recorded 2026-11-27 (Linux cloud container, provisional) from the stock 3.10.0 release tarball (SHA-512 checked, security plugin removed), single node.

- AC-1: `an_assembled_node_answers_every_breadth_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs) replays `size-zero-aggs.json`, 12 searches. Seen red before the code: 7 steps failed with the missing `terminated_early`. NOT-RUN: `terminate_after` and `min_score` beside aggregations (the node refuses them), a one-word `match`.
- AC-2: `a_size_zero_search_with_an_aggregation_is_terminated_early_as_stock_marks_it` (crates/reader/src/search_tests.rs).
- AC-3: `an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did` (crates/compat/tests/node_cluster.rs). A first version that wrote the flag for every size 0 aggregation failed it on 11 steps (seen red), which is how the nested-mapping condition was found.
