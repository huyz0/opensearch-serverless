# Reader and coordinator: sibling pipeline aggregations

## Serves

Roadmap R10.2: the pipeline aggregations every dashboard asks for, after the bucket aggregations they read ([terms-agg](../terms-agg/SPEC.md), [histogram-agg](../histogram-agg/SPEC.md), [date-histogram-agg](../date-histogram-agg/SPEC.md), [range-agg](../range-agg/SPEC.md)). Of the pipeline aggregations OpenSearch 3.10.0 registers this is the first slice: `avg_bucket`, `sum_bucket`, `min_bucket`, `max_bucket` and `stats_bucket` at the top level of `aggs`. The parent pipelines (`derivative`, `cumulative_sum`, `bucket_script`, `bucket_selector`, `bucket_sort`) and the forwarded wire are the next slices.

## Change

A pipeline aggregation is not collected by a shard. It is parsed into its own list on the search source (`SearchSource.pipelines`), checked as `AggregatorFactories.Builder.validate` checks it, and computed after the final reduce, in the writer of `aggregations` both the single-node reader and the coordinator use: the writer renders the aggregation the `buckets_path` names, reads that body back, computes the value as `BucketMetricsPipelineAggregator.doReduce` does and lists the result after the aggregations a reduce lists, in request order (as first written this listed it among them by `HashMap` order, which Java does not; [pipeline-aggs-parent](../pipeline-aggs-parent/SPEC.md) corrected it, and the empty-bucket gap below). The shard, the shard reduce and the forwarded request are unchanged: a Rust coordinator keeps the pipelines to itself.

Body: `buckets_path` (a string, or an array of one string) and `gap_policy` (`skip`, the default, or `insert_zeros`; a bucket with no value is skipped or counts 0). A path is `agg>_count`, `agg>sub` or `agg>sub.metric` over a `terms`, `histogram`, `date_histogram`, `range` or `date_range` at the top level and a single-value metric, or one member of a `stats`, beneath it. Result shapes: `avg_bucket` and `sum_bucket` write `value` (`simple_value`), `min_bucket` and `max_bucket` write `value` and the `keys` of every bucket holding it (`bucket_metric_value`), `stats_bucket` writes `count`, `min`, `max`, `avg` and `sum` (`stats_bucket`); a value that is not finite is `null`.

Not in this change, each refused as "not supported by this node yet": `format`; more than one `buckets_path`; a path of other shapes (the aggregation alone, three elements, `_key`, a bracketed key); a pipeline under another aggregation; a bucket aggregation other than the five above as the input; a sub-aggregation of another type than the single-value metrics and `stats`. The rendered input is read back whole, so one over 8 MiB fails the search.

## Acceptance criteria

- AC-1: over four buckets with one gap, each of the five kinds over `agg>sub` and `agg>_count` with each gap policy writes the body 3.10.0 writes (`fixtures/pipeline-aggs/java-pipeline-3.10.0.txt`).
- AC-2: equal minimums or maximums list every key in bucket order; with no buckets `avg_bucket` is `null`, `sum_bucket` 0.0, `min_bucket` and `max_bucket` `null` with no keys, and `stats_bucket` has count 0, `null` extremes and sum 0.0.
- AC-3: keyed bucket bodies (an object of buckets) are read by their member names, a bucket's key is its `key_as_string` when written and else its `key` as written, and `sub.metric` reads that member of the sub-aggregation's body.
- AC-4: a body is parsed as 3.10.0's parser parses it: a missing `buckets_path`, a token of another type, an unknown member and an invalid gap policy fail with the recorded messages (`java-parse-3.10.0.txt`).
- AC-5: a path is validated as the request validates it, with the recorded `Validation Failed: 1: …;` messages: no such aggregation, no such sub-aggregation, the cyclical path, and a first aggregation that is not multi-bucket (`java-validation-3.10.0.txt`).
- AC-6: with `typed_keys` a result's key is `simple_value#name`, `bucket_metric_value#name` or `stats_bucket#name`.
- AC-7: the coordinator computes the pipeline over the reduced buckets of several shards.
- AC-8: a search over a REST node writes the pipeline aggregations and refuses a path it validates against.

## Test plan

`crates/reader/src/aggs/pipeline/pipeline_tests.rs` (AC-1 to AC-6), `crates/coordinator/tests/it/search.rs` (AC-7) and `crates/node/tests/it/search_pipelines.rs` (AC-8). The recorders `PipelineRecordings.java`, `PipelineValidation.java` and `PipelineParse.java` compile against the OpenSearch 3.10.0 jars as `fixtures/search-shard/CardRecordings.java` does, and run the real builders and aggregators.

## S3 request budget

Zero: the pass reads the response of the aggregation, not the index. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with a pipeline aggregation was refused before, and a request without one is unchanged.

## Risks

The values are Java's own for the buckets recorded; a bucket key a histogram writes as a double is listed as the renderer writes it, which is Java's `Double.toString` form. The input is the aggregation's JSON, read back, so a `terms` over many buckets costs a parse of its response; the 65,535-bucket limit bounds it. The tests were written after the code and not seen red first.
