# Forwarded pipeline aggregations

## Serves

Roadmap R10.2: the pipeline aggregations of the REST path (`pipeline-aggs-sibling`, `pipeline-aggs-parent`) were refused on a Java coordinator's forwarded shard ask (`internal:serverless/search/shard`) by name ("pipeline aggregations"), so a search with `avg_bucket` or `derivative` over an index a Rust node writes failed with a 503 instead of running. A shard never computes a pipeline aggregation: the coordinator runs it after the final reduce, from the reduced aggregations. What the Rust owner owes is to read the builders, answer the shard-level aggregations as before and not refuse.

## Change

`oss-transport` reads the pipeline list of each `AggregatorFactories.Builder` (the top level, and the sub-builder of a bucket aggregation) as the Java shell writes it: the writeable name, the name, `bucketsPaths` as a string array, a null metadata map, then the builder's own fields: `avg_bucket`, `sum_bucket`, `min_bucket`, `max_bucket` and `stats_bucket` an optional format and the gap policy; `derivative` an optional format, a flag, the gap policy and an optional unit; `cumulative_sum` an optional format; `bucket_sort` its sorts (field, no nested filter, path or sort, no `missing`, an optional order, no mode, unmapped type or numeric type), `from`, an optional `size` and the gap policy. The reader keeps them in the request as `Source::pipelines`, each with the names of the aggregations that hold it, and the request writes back the same bytes. `oss-node` ignores them: the shard answer is the aggregations' answer.

Refused by name, as before: a pipeline aggregation of another type (`percentiles_bucket`, `bucket_script`, `moving_fn`, ...), metadata, a sort's nested filter, nested path or sort, `missing`, mode, unmapped type or numeric type, and more than 64 pipeline aggregations in a request.

## Acceptance criteria

- AC-1: the pipeline list of a request reads, and a request written from the read one is the same bytes, for the five sibling kinds (with and without a format and a gap policy), `derivative` (with and without a gap policy, format and unit), `cumulative_sum` and `bucket_sort` (with sorts, `from`, `size`; with none).
- AC-2: the bytes are the ones OpenSearch 3.10.0's own `AggregatorFactories.Builder` writes (`fixtures/search-shard/java-pipeline-wire-3.10.0.txt`, from `PipelineWireRecordings.java`).
- AC-3: a pipeline under a bucket aggregation is kept with the names of its hosts; one at the top level has none; several are kept in request order.
- AC-4: another pipeline type, a sort option listed above, and 65 pipeline aggregations are refused by name.
- AC-5: a forwarded search whose request holds pipeline aggregations is answered with the shard-level aggregations alone, the bytes it answers without them.

## Test plan

Unit tests in `oss-transport` (`aggs_pipeline_tests.rs`: AC-1, AC-3, AC-4), the Java recordings in `crates/transport/tests/search_shard_pipeline.rs` (AC-1, AC-2) and `oss-node` (`search_shard_action/tests/pipelines.rs`: AC-5).

## S3 request budget

None: the shard search is unchanged. LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before; a request without pipeline aggregations is unchanged.

## Risks

A pipeline kind the Java coordinator sends that is not listed is refused as before, and the search fails there rather than returning a wrong answer.
