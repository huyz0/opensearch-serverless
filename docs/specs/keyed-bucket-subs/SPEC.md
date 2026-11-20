# Reader: sub-aggregations of `histogram` and `date_histogram`

## Serves

Roadmap R10.2: sub-aggregations under the bucket aggregations whose buckets come from the data, the second of the series begun by [fixed-bucket-subs](../fixed-bucket-subs/SPEC.md) (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` accepts `aggs` under a `histogram` or `date_histogram`: a single-value or stats metric, or a keyword `terms`, as under a `range`. Each sub-aggregation's engine node goes under the parent's node in lucene-aggs (`BucketsAggregator.buildSubAggsForBuckets`), and the engine's result gives each kept bucket its list of values (`aggs::bucket::keyed_subs`, on `keyword_subs`' and `sub_values`' code). A result keeps them beside its buckets as `BucketSubs`: each sub-aggregation's name and format, one list of values a bucket, and what each holds for a bucket with no document. A reduce keeps each key's values through the merge and the order of `InternalHistogram.reduce` and `InternalDateHistogram.reduce`: the values of equal keys reduce together as the sub-aggregations reduce at the top level, and a bucket the reduce fills in (`min_doc_count` 0) gets the empty values, as OpenSearch's empty bucket holds empty sub-aggregations. Each bucket writes its sub-aggregations after `doc_count`. A `histogram`, `range` or `filters` under a `histogram`, and a `terms` over a numeric field, are refused as "not supported by this node yet".

## Acceptance criteria

- AC-1: each bucket of a `histogram` or `date_histogram` holds the metric sub-aggregations of the documents it counts.
- AC-2: a keyword `terms` under a bucket counts that bucket's documents.
- AC-3: a histogram over an unmapped field has no bucket, and its sub-aggregations the empty values for a reduce to fill with.
- AC-4: a sub-aggregation of a kind this node does not run under a bucket aggregation fails with a 400 naming it, never silently dropped.
- AC-5: shard results reduce each key's values as the metrics and `terms` reduce at the top level, keep them with their bucket under any order, and fill the gaps with the empty values; the coordinator's `reduce_shards` serves them unchanged.
- AC-6: the response writes each bucket's sub-aggregations after its `doc_count`, in the order OpenSearch lists them.
- AC-7: a search reserves the sub-aggregations' states and values from the request-body pool with its hits: a bucket holds a value of its field at least (a document with several values lands in several buckets), so at most the field's values, which the segments state, and the bucket limit of them.
- AC-8: no request is added (the row of [request-budgets.md](../../request-budgets.md) for the term search with aggregations, whose request now holds a `histogram` with a sub-aggregation).

## Test plan

Unit tests in `aggs/bucket/keyed_subs_tests.rs` over an `oss-testkit` index and on hand-made results (AC-1 to AC-7), `crates/coordinator/tests/histogram_reduce.rs` (AC-5 across shards), the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8). Expected values come from reading `BucketsAggregator`, `InternalHistogram` and `InternalDateHistogram` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged (AC-8): LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with sub-aggregations under these was refused before, and one without is unchanged.

## Risks

The coordinator's reduce holds each key's values from every shard and clones the empty values into the buckets it fills, at most the shards times the bucket limit; it is not reserved, as the reduce of the buckets themselves is not.

No stock 3.10.0 recording backs these steps. A reduce clones the empty values into each bucket it fills, at most the bucket limit of them, as the buckets themselves are. The `date_histogram` has no test of its own over a date-typed field's text keys beside the sub-aggregations (its keys render as before).
