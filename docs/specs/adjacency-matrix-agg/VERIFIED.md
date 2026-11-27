# Verified: adjacency_matrix aggregation

- AC-1: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/bucket/adjacency_tests.rs
- AC-2: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/bucket/adjacency_tests.rs
- AC-3: `each_filter_and_each_pair_of_filters_counts_the_documents_it_matches` in crates/reader/src/aggs/bucket/adjacency_tests.rs
- AC-4: `a_separator_names_the_pairs_and_only_intersections_leaves_out_the_filters` in crates/reader/src/aggs/bucket/adjacency_tests.rs
- AC-5: `only_the_buckets_with_documents_are_written` in crates/reader/src/aggs/bucket/adjacency_tests.rs
- AC-6: `an_adjacency_matrix_counts_each_filter_and_each_pair` in crates/node/tests/it/search_pipelines.rs
- AC-7: `a_sub_aggregation_is_refused` in crates/reader/src/aggs/bucket/adjacency_tests.rs

The recordings (`fixtures/adjacency-agg`) are the 3.10.0 builder's parse of 23 bodies and `InternalAdjacencyMatrix.reduce` over shard results. The tests were written after the code. The full gate, coverage and mutation are left to CI.
