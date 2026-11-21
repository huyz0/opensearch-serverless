# Reader: sub-aggregations of `range`, `filters` and `filter`

## Serves

Roadmap R10.2: sub-aggregations under the bucket aggregations whose buckets are fixed by the request ([range-agg](../range-agg/SPEC.md), [filters-agg](../filters-agg/SPEC.md), [filter-agg](../filter-agg/SPEC.md)), the first of the series that serves them under every bucket aggregation (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` accepts `aggs` under a `range`, `filters` or `filter`, as it does under a keyword `terms`: a single-value or stats metric, or a keyword `terms`. Each sub-aggregation's engine node goes under the parent's `range` or `filters` node in lucene-aggs (`BucketsAggregator.buildSubAggsForBuckets`), the engine's fixed buckets give each bucket its list of values (`aggs::bucket::fixed_subs`), shard results reduce bucket for bucket as `InternalAggregations.reduce` does, and each bucket writes its sub-aggregations after `doc_count` in `Aggregations.toXContentInternal`'s order (`aggs::bucket::sub_values`, shared with the keyword `terms`, whose own code now uses it). Any other sub-aggregation (a `histogram`, `range`, `filters`, ...) is refused as "not supported by this node yet", as is a `terms` of a numeric field.

## Acceptance criteria

- AC-1: each bucket of a `filters` holds the metric sub-aggregations of the documents it counts, the other bucket those of the documents matching no filter; a `range`'s buckets and a `filter`'s one bucket likewise.
- AC-2: a keyword `terms` under a bucket counts that bucket's documents.
- AC-3: a bucket aggregation over an unmapped field has every bucket's sub-aggregations empty, as a metric over no value is.
- AC-4: a sub-aggregation of a kind this node does not run under a bucket aggregation fails with a 400 naming it, never silently dropped (a numeric `terms` and a `range`, `histogram` or `date_histogram` are served since [nested-bucket-aggs](../nested-bucket-aggs/SPEC.md); `filters` and `filter` still fail).
- AC-5: shard results reduce each bucket's sub-aggregation values as the metrics and `terms` reduce at the top level; the coordinator's `reduce_shards` serves them unchanged.
- AC-6: the response writes each bucket's sub-aggregations after its `doc_count` (`range` after its `to`), and a `filter` writes them beside `doc_count`, in the order OpenSearch lists them.
- AC-7: a search reserves the sub-aggregations' states and values from the request-body pool with its hits: each bucket's, and a nested `terms`' under the buckets; the number of buckets stays bounded by the bucket limit.
- AC-8: no request is added: sub-aggregations read the same doc values and ordinals through the same page cache as the same aggregations at the top level (the row of [request-budgets.md](../../request-budgets.md) for the term search with aggregations).

## Test plan

Unit tests in `aggs/bucket/fixed_subs_tests.rs` over an `oss-testkit` index (AC-1, AC-2, AC-3, AC-4, AC-6, AC-7), `aggs/bucket/filters_tests.rs` and `range_tests.rs` (AC-4 at parse), `crates/coordinator/tests/{filters,range}_reduce.rs` (AC-5); the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8). Expected values come from reading `BucketsAggregator`, `InternalFilters`, `InternalRange` and `InternalAggregations` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged (AC-8): LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with sub-aggregations under these was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. A `terms` under a bucket holds engine state for every (bucket, term) pair, at most the bucket limit; its bytes are reserved with the aggregation's, as a nested `terms` under a `terms` is.
