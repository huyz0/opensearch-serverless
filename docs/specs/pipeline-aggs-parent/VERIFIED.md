# Verified: parent pipeline aggregations

- AC-1: `derivative_and_cumulative_sum_give_the_bytes_java_wrote` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-2: `a_bucket_with_no_document_is_a_gap_unless_the_path_is_count` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-3: `bucket_sort_orders_trims_and_drops_as_java_does` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-4: `results_follow_the_sub_aggregations_in_the_order_the_pipelines_run`, `typed_keys_name_a_result_by_its_type` and `keyed_buckets_are_changed_in_place` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-5: `sibling_results_are_listed_after_the_aggregations_a_reduce_lists` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-6: `a_parent_pipeline_is_validated_as_java_validates_it` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-7: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-8: `a_parent_pipeline_under_a_nested_aggregation_or_a_sibling_one_in_a_bucket_is_refused` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-9: `the_parent_pipelines_of_a_request_are_kept_with_their_host` in crates/reader/src/aggs/pipeline/parent_tests.rs
- AC-10: `a_parent_pipeline_aggregation_works_on_the_buckets_that_hold_it` in crates/node/tests/it/search_pipelines.rs

The recordings (`fixtures/pipeline-aggs`) are the 3.10.0 builders and aggregators run over an `InternalHistogram` of four buckets, the body parsers and request validation run over the recorded cases, and `InternalAggregations.topLevelReduce` run over one and two shards' results. The tests passed at their first run, written after the code. Tests passed here: oss-reader `aggs::pipeline`; the node test named above is listed below once it ran; the full gate, coverage and mutation are left to CI.
