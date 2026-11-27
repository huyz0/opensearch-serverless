# Verified: forwarded pipeline aggregations

Recorded on 2026-11-27 against the 3.10.0 jars from Maven Central: `PipelineWireRecordings.java` wrote `java-pipeline-wire-3.10.0.txt` (the five sibling kinds, with and without a format and gap policy; `derivative` and `cumulative_sum` under a `date_histogram`, with and without options; `bucket_sort` with sorts and with a window only; two siblings in one request). The Rust bytes written from the read ones matched at the first run. `javap` on `DerivativePipelineAggregationBuilder.doWriteTo`, `BucketSortPipelineAggregationBuilder.doWriteTo` and `FieldSortBuilder.writeTo` confirmed the field order the recordings show (the derivative's flag before its gap policy).

- AC-1: `the_sibling_kinds_read_from_java_bytes`, `a_format_and_a_gap_policy_read_from_java_bytes`, `a_derivative_reads_under_its_host`, `a_cumulative_sum_reads_from_java_bytes`, `a_bucket_sort_reads_from_java_bytes` (`crates/transport/tests/search_shard_pipeline.rs`; each also writes the bytes back)
- AC-2: `the_sibling_kinds_read_from_java_bytes`, `a_derivative_reads_under_its_host`, `a_bucket_sort_reads_from_java_bytes` (over `java-pipeline-wire-3.10.0.txt`)
- AC-3: `a_derivative_reads_under_its_host`, `several_pipelines_keep_their_request_order`
- AC-4: `another_pipeline_type_is_refused_by_name`, `metadata_is_refused_by_name`, `a_sort_option_is_refused_by_name`, `a_gap_policy_that_is_not_one_is_malformed`, `more_than_sixty_four_pipelines_are_refused_before_any_is_read`
- AC-5: `pipeline_aggregations_do_not_change_the_shards_answer` (`oss-node`, `search_shard_action/tests/pipelines.rs`)

Not run locally: the full gate, coverage and mutation (left to CI).
