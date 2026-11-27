# Verified: sibling pipeline aggregations

- AC-1: `every_kind_gives_the_bytes_java_wrote_for_both_gap_policies_and_paths` in crates/reader/src/aggs/pipeline/pipeline_tests.rs
- AC-2: `ties_list_every_key_and_no_buckets_give_null_or_zero` in crates/reader/src/aggs/pipeline/pipeline_tests.rs
- AC-3: `keyed_buckets_and_date_keys_are_read_as_java_reads_them` in crates/reader/src/aggs/pipeline/pipeline_tests.rs
- AC-4: `bad_bodies_fail_as_java_fails` in crates/reader/src/aggs/pipeline/pipeline_tests.rs
- AC-5: `a_path_is_validated_as_java_validates_it` in crates/reader/src/aggs/pipeline/pipeline_tests.rs
- AC-6: `a_sibling_pipeline_aggregation_is_computed_over_the_reduced_buckets` in crates/coordinator/tests/it/search.rs, `a_sibling_pipeline_aggregation_is_written_beside_the_aggregation_it_reads` in crates/node/tests/it/search_pipelines.rs
- AC-7: `a_sibling_pipeline_aggregation_is_computed_over_the_reduced_buckets` in crates/coordinator/tests/it/search.rs
- AC-8: `a_sibling_pipeline_aggregation_is_written_beside_the_aggregation_it_reads` in crates/node/tests/it/search_pipelines.rs

The recordings (`fixtures/pipeline-aggs`) are the 3.10.0 builders and aggregators run over a `LongTerms` of four buckets, and the request validation and body parser run over the recorded cases. The tests passed at their first run, written after the code. Tests passed here: oss-reader `aggs::pipeline`, the coordinator and node tests named above; the full gate, coverage and mutation are left to CI.
