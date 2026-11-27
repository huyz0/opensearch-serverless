# bucket_script and bucket_selector pipeline aggregations

## Serves

Roadmap R10.2: the pipeline aggregations that run a script. `bucket_script` adds a value to each bucket of the histogram, date_histogram or terms aggregation that holds it; `bucket_selector` drops the buckets its script rejects. Both are parent pipelines like `derivative` ([pipeline-aggs-parent](../pipeline-aggs-parent/SPEC.md)) and run in the same post-pass over the host's rendered buckets. Their scripts run on this node's Painless, so both are served only when the node compiles Painless (`serverless.scripting.painless.native_compile`, [ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md)); with it off they are refused as before. `moving_fn` stays refused (it needs `MovingFunctions` in Painless).

## Change

`oss-reader` parses `bucket_script` as `BucketScriptPipelineAggregationBuilder.PARSER` does (an object parser: `buckets_path` as an object, an array, or one path; `script` as text or an object; `gap_policy`) and `bucket_selector` as its hand-written token loop, each with the messages Java prints. Each bucket's variables (`buckets_path` entries: `_count`, a sub-aggregation, `sub.metric`) are resolved as the other parent pipelines resolve a path, and join the script's own params (the variables win). The script runs as a Painless `reduce` script (`params` bound, a `return` for the value).

`bucket_script`: a variable that is a gap under `skip` (NaN, infinite, or no documents for a path other than `_count`) skips the bucket; `insert_zeros` makes it 0. A script that returns `null` adds nothing; a number is written as a double (`{"value":v}`, `null` for NaN or infinite). The result is listed after the bucket's sub-aggregations in execution order, `simple_value#name` with `typed_keys`. A `buckets_path` that names no sub-aggregation is not validated, as in Java: the variable is a gap.

`bucket_selector`: keeps the buckets whose script returns `true`. A gap is not skipped: the variable is NaN under `skip` and 0 under `insert_zeros`.

Refused by name: `format` (no `DecimalFormat` port), `lang` other than painless, a `buckets_path` that goes through `>` or reads `_key`, and a `buckets_path` array holding a non-text element. A script object's own errors are worded by the shared script parser, not by Java.

Known differences: a script is compiled for each bucket, so a script that does not compile fails the search only when the host has a bucket; a failing script is a 500 `io` error here and a 400 `script_exception` in Java; the forwarded shard wire does not carry these builders (a forwarded search holding one is refused).

## Acceptance criteria

- AC-1: both pipelines over the recorder's histogram give the bytes Java 3.10.0 wrote for each path (`s`, `a` with a gap, `_count`), gap policy, a gap in the data and a bucket with no documents (`fixtures/pipeline-script/java-script-3.10.0.txt`, 94 recordings of results).
- AC-2: the result forms Java wrote: a ratio, params, an int, a long (`5.0E9`), NaN (`null`), a `null` result (no value), no variables, an unknown path, `sub.value`, and the selector's `true`, `false`, `_count` and empty-variable cases.
- AC-3: with `typed_keys` the `bucket_script` result is `simple_value#name`.
- AC-4: a script that throws, does not compile, or returns the wrong type fails the search with a 400 `script_exception` naming the pipeline, before the response starts (single-shard and coordinated searches, and `_msearch` items).
- AC-5: a body is read, and refused, with the messages Java prints, for every body recorded.
- AC-6: a parsed pipeline keeps its script, params, variables, gap policy and host.
- AC-7: with the switch off both are refused; at the top level they fail validation as Java does (`must be declared inside of another aggregation`); the refused shapes above answer "not supported by this node yet".
- AC-8: over a REST search on a node with Painless a `bucket_script` adds a value, a `bucket_selector` keeps or drops buckets, and a bad gap policy is a 400.

## Test plan

`oss-reader` unit tests (`pipeline/script_tests.rs`: AC-1 to AC-7) and the node integration test `the_scripts_of_two_pipelines_run_over_the_buckets` (AC-8).

## S3 request budget

None: a post-pass over a rendered body. LIST 0, HEAD 0, PUT 0.

## Switch

The existing `native_compile` switch, off by default: with it off a request holding either aggregation is refused as before.

## Risks

Compiling the script for each bucket costs time on a host with many buckets (bounded by the bucket limit).
