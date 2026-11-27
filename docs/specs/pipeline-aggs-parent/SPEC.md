# Reader and coordinator: parent pipeline aggregations

## Serves

Roadmap R10.2, second slice after [pipeline-aggs-sibling](../pipeline-aggs-sibling/SPEC.md): the pipeline aggregations that work inside a bucket aggregation, `derivative`, `cumulative_sum` and `bucket_sort`, over a `histogram` or `date_histogram` (`bucket_sort` also over `terms`, `range` and `date_range`) at the top level of `aggs`. `bucket_script`, `bucket_selector` and `moving_fn` need Painless, which has no Rust front end yet, and the forwarded wire is the next slice.

## Change

A parent pipeline is written inside the `aggs` of the bucket aggregation it works on. It is not collected by a shard: the parser keeps it on the search source's `pipelines` list with its `host` set, the bucket aggregation's own `aggs` stay free of it, and the writer of `aggregations` (shared by the reader and the coordinator) applies it after the final reduce. The writer renders the host's body, reads it back, runs the pipelines on its buckets and writes the result, as `PipelineAggregator.reduce` of each does. The shard, the shard reduce and the forwarded request are unchanged.

- `derivative`: from the second bucket on, `{"value": this − previous}` after the bucket's sub-aggregations; the previous value is kept even when it is a gap, so a gap makes the difference after it `null` too under `skip`. Body: `buckets_path` (a string, or an array of which the first is used), `gap_policy`.
- `cumulative_sum`: `{"value": running sum}` in every bucket, a bucket with no value adding nothing. Body: `buckets_path` (one path).
- `bucket_sort`: drops buckets for which every sort value is missing, orders the rest by the sorts (a bucket missing a value sorts after those that have it, whatever the order, and ties keep the bucket order), then keeps `size` buckets from `from`. With no sorts the window alone applies. Body: `sort` (field names, ascending, or objects of field to `asc` or `desc`, either in an array or alone), `from`, `size`, `gap_policy` (case-insensitive here).
- A path is a bucket's `_count`, `sub` or `sub.metric` of a single-value metric or `stats` member, or the name of another parent pipeline of the same host, which then runs first (`bucket_sort` also reads `_key`, a number or a keyword key). A gap is a value that is missing, NaN or infinite, or a bucket with no document read by anything but `_count` (`BucketHelpers.resolveBucketValue`); `skip` leaves it out, `insert_zeros` counts it 0. `cumulative_sum` always counts a gap 0.
- Results are listed after the bucket's sub-aggregations in the order the pipelines run, as `InternalAggregations` appends them; with `typed_keys` a result's key is `simple_value#name`. Sibling pipeline results are likewise appended after the top-level aggregations, in request order; slice one listed them among the others by `HashMap` order, which Java does not (`fixtures/pipeline-aggs/java-order-3.10.0.txt`), and read an empty bucket's `sub` as 0, where Java treats it as a gap. Both are corrected here.

Not in this change, each refused as "not supported by this node yet": `format` and `unit`; a path of other shapes (`a>b` where the first element is not a leaf metric, `_key` for `derivative` and `cumulative_sum`); more than one `buckets_path` for `cumulative_sum`; a pipeline named as another aggregation of the same host; a parent pipeline under a host that is itself nested; a sibling pipeline inside a bucket; a bucket sort's `from` or `size` beyond 2^31; pipelines that read one another in a cycle. A sibling pipeline reading a parent pipeline's result is not served either (it validates as "No aggregation [d] found"). Where Java accepts a path to nothing and writes nothing or zero, validation refuses it first, as it does here.

## Acceptance criteria

- AC-1: over four buckets, with and without a bucket whose `avg` has no value, `derivative` over `sub`, `_count` and an `avg` with each gap policy, and `cumulative_sum` over the same, write the body 3.10.0 writes (`fixtures/pipeline-aggs/java-parent-3.10.0.txt`).
- AC-2: a bucket with no document is a gap for every path but `_count`, for `derivative`, `bucket_sort` and the sibling pipelines alike (recorded `empty.*` cases).
- AC-3: `bucket_sort` orders by `sub`, `_count` and `_key`, ascending and descending, by several sorts, with `from` and `size`, with no sorts, and drops or keeps buckets with a gap by the gap policy, as recorded.
- AC-4: results follow the sub-aggregations in the order the pipelines run, a pipeline that reads another runs after it, `typed_keys` names a result `simple_value#name`, and keyed buckets are changed in place.
- AC-5: sibling pipeline results are listed after the aggregations a reduce lists, in request order (recorded `shards1`).
- AC-6: a parent pipeline is validated as the request validates it, with the recorded messages: a path to nothing, the cyclical path, a path under a leaf, a `derivative` or `cumulative_sum` under a host that is not a histogram, a pipeline at the top level.
- AC-7: a body is parsed as 3.10.0's parsers parse it: each recorded failure has the exception type and message chain Java gives (`java-parent-parse-3.10.0.txt`), every recorded success parses or is refused as not supported.
- AC-8: the shapes listed above as not supported fail with that message.
- AC-9: a request's parent pipelines are kept in order with their host, the host's own `aggs` hold none of them, and a `bucket_sort` keeps its sorts, window and gap policy.
- AC-10: a search over a REST node writes the parent pipelines typed or not, trims by a `bucket_sort`, and refuses a path to nothing.

## Test plan

`crates/reader/src/aggs/pipeline/parent_tests.rs` (AC-1 to AC-9) and `crates/node/tests/it/search_pipelines.rs` (AC-10). The recorders `PipelineParent.java`, `PipelineParentParse.java` and `PipelineOrder.java` compile against the OpenSearch 3.10.0 jars as `PipelineRecordings.java` does and run the real builders, aggregators, parsers, request validation and final reduce.

## S3 request budget

Zero: the pass reads the response of the aggregation, not the index. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with a parent pipeline was refused before, a request without one is unchanged, and the two corrections to the sibling pipelines make them match Java where they did not.

## Risks

A `bucket_sort`'s cause chain for a `from` or `size` that fails after `sort` is unwrapped, and before it is wrapped in "failed to build" with the last failing one reported, as recorded for the cases tried; other orders are inferred. The locations in parse messages are those of the whole request, where the recorder parsed the body alone, so the tests compare them without. A `_key` sort compares a number key as a double. The tests were written after the code and not seen red first.
