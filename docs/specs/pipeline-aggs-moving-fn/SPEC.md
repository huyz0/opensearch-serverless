# moving_fn pipeline aggregation

## Serves

Roadmap R10.2: the last pipeline aggregation that runs a script. `moving_fn` sits inside a `histogram` or `date_histogram` and adds to each bucket the result of a script over a window of the preceding values of `buckets_path`. It is a parent pipeline like `derivative` ([pipeline-aggs-parent](../pipeline-aggs-parent/SPEC.md)) and, like [bucket_script and bucket_selector](../pipeline-aggs-script/SPEC.md), runs the script on this node's Painless and is served only when the node compiles Painless (`serverless.scripting.painless.native_compile`, off by default).

## Change

`MovingFunctions` (`max`, `min`, `sum`, `stdDev`, `unweightedAvg`, `linearWeightedAvg`, `ewma`, `holt`, `holtWinters`) is in lucene-rust's Painless (huyz0/lucene-rust#612, checked against 899 recorded calls); this repository's lucene-rust pin moves to it.

`oss-reader` parses `moving_fn` as `MovFnPipelineAggregationBuilder.PARSER` does: `buckets_path` (a string), `script` (text or an object) and `window` (an integer; text holding one is read, a fraction truncated) are required, `gap_policy`, `shift` and `format` optional, with the messages Java prints (including `[window] must be a positive, non-zero integer.`, raised when the last required field arrives). It is validated as Java validates it: inside a histogram or date_histogram, or `moving_fn aggregation [p] must have a histogram, date_histogram or auto_date_histogram as parent`; the path is not checked against the host (Java does not), so a path that names nothing leaves the buckets as they were.

Computed as `MovFnPipelineAggregator.reduce`: the series is the host's buckets that have a value (a gap is skipped under `skip` and 0 under `insert_zeros`); the bucket at position `i` of the series gets the script over series values `i - window + shift` up to `i + shift` (clamped to the series), as `{"value": v}` after its sub-aggregations, `simple_value#name` with `typed_keys`, `null` for NaN; a bucket with no value is left as it is. The script sees `values`, a `double[]` of the window, and `params`; it is run as a Painless reduce script with `values` declared ahead of its source from a `params._mf_values` list, so `params` also holds that key. A script that fails, or returns something other than a number, fails the search with a 400 `script_exception` naming the pipeline (the same settle step as the other script pipelines).

Refused by name: `format`, `lang` other than painless, a `buckets_path` through `>` or reading `_key`.

## Acceptance criteria

- AC-1: for every recorded reduce (histograms with no gap, a gap and a bucket with no documents; paths `s`, `a`, `_count`; both gap policies; windows 1 to 10; shifts -1, 0, 1; scripts over `MovingFunctions`, `values.length` and a param) the body is the bytes Java 3.10.0 wrote (`fixtures/pipeline-script/java-moving-3.10.0.txt`).
- AC-2: a path that names nothing leaves the buckets as they were.
- AC-3: with `typed_keys` the result is `simple_value#name`.
- AC-4: a script that throws fails the search naming the pipeline.
- AC-5: a body is read, and refused, with the messages Java prints, for every body recorded.
- AC-6: a parsed pipeline keeps its window, shift, script, gap policy and host.
- AC-7: with the switch off it is refused; at the top level and under a non-histogram it fails validation as Java does; the refused shapes answer "not supported by this node yet".
- AC-8: over a REST search on a node with Painless a `moving_fn` answers, and a script that throws is a 400.

## Test plan

`oss-reader` unit tests (`pipeline/moving_tests.rs`: AC-1 to AC-7) and the node integration test `a_moving_fn_runs_over_the_buckets` (AC-8).

## S3 request budget

None: a post-pass over a rendered body. LIST 0, HEAD 0, PUT 0.

## Switch

The existing `native_compile` switch, off by default.

## Risks

The script is compiled for each bucket. A window script sees an extra `params._mf_values` key. The Painless runs are not recorded against Java's Painless: the recorder's test engine calls `MovingFunctions` from Java, and the runtime is checked against the same class separately in lucene-rust.
