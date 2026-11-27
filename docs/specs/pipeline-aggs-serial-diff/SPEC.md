# serial_diff pipeline aggregation

## Serves

Roadmap R10.2: the last pipeline aggregation that needs no script. `serial_diff` sits inside a `histogram`, `date_histogram` or `auto_date_histogram` and adds to each bucket the difference between its value and the value `lag` buckets before. It is a parent pipeline like `derivative` ([pipeline-aggs-parent](../pipeline-aggs-parent/SPEC.md)) and runs in the same post-pass over the host's rendered buckets; `bucket_script`, `bucket_selector` and `moving_fn` need Painless and stay refused.

## Change

`oss-reader` parses `serial_diff` as `SerialDiffPipelineAggregationBuilder.parse` does: a token loop with `buckets_path` (text, or an array whose first element is used), `gap_policy` (`skip` or `insert_zeros`, exact case), `lag` (a number, a fraction truncated, at least 1; default 1) and the messages Java prints, including its copy of `derivative`'s message for a missing `buckets_path`. It validates as `derivative` does (parent kind, path exists, no cycle). Each bucket at position `i` of at least `lag` gets `{"value": v[i] - v[i-lag]}` when both values exist; a bucket whose value is a gap (NaN, infinite, or no documents for a path other than `_count`; `insert_zeros` makes it 0) counts as a position and has no result where it, or the bucket `lag` before, is a gap. The result is listed after the bucket's sub-aggregations in execution order, `simple_value#name` with `typed_keys`. `oss-transport` reads and writes the builder on a forwarded shard search ([forwarded-pipeline-aggs](../forwarded-pipeline-aggs/SPEC.md)): the writeable `serial_diff`, an optional format, the gap policy and the `lag`.

Refused by name: `format` (no `DecimalFormat` port yet), a `lag` beyond 2^31, a `buckets_path` array that is empty or holds a non-text, non-number element.

## Acceptance criteria

- AC-1: `serial_diff` over the recorder's histogram gives the bytes Java 3.10.0 wrote for every path (`s`, `_count`, an `avg` with a gap), lag (1, 2, 3, 5), gap policy and a gap in the data (`fixtures/pipeline-aggs/java-serial-3.10.0.txt`, 48 recordings).
- AC-2: a bucket with no documents is a gap for a path other than `_count`, for either policy and lags 1 and 2.
- AC-3: with `typed_keys` the result is `simple_value#name`.
- AC-4: a body is read, and refused, with the messages Java prints, for every body recorded (`java-serial-parse-3.10.0.txt`).
- AC-5: validation as Java does: not under a `terms`, a missing path, a cycle, a name clash, a path through another pipeline, at the top level.
- AC-6: over a REST search a `serial_diff` in a histogram answers, typed or not; `lag` 0 and a `format` are refused with a 400.
- AC-7: a forwarded shard search holding a `serial_diff` reads it from the bytes Java wrote, with and without options, and writes them back (`fixtures/search-shard/java-pipeline-wire-3.10.0.txt`).

## Test plan

`oss-reader` unit tests (`pipeline/serial_tests.rs`: AC-1 to AC-5), the node integration test `a_serial_diff_works_on_the_buckets_that_hold_it` (AC-6) and `crates/transport/tests/search_shard_pipeline.rs` (AC-7).

## S3 request budget

None: a post-pass over a rendered body. LIST 0, HEAD 0, PUT 0.

## Switch

None: the aggregation was refused before; a request without it is unchanged.

## Risks

`format` is refused, so a request that sets it fails rather than answering without `value_as_string`.
