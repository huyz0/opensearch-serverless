# Reader: `histogram` and `date_histogram` ordered by a sub-aggregation

## Serves

Roadmap R10.2: the `order` of a `histogram` or `date_histogram` by a metric sub-aggregation (`{"order": {"m": "desc"}}`, `{"order": {"s.max": "asc"}}`), which [histogram-agg](../histogram-agg/SPEC.md) and [date-histogram-agg](../date-histogram-agg/SPEC.md) refused. A single-shard `_search` with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

The parser reads an `order` key that is neither `_count` nor `_key` as the path of a sub-aggregation (`AggregationPath.parse`, as [terms-agg](../terms-agg/SPEC.md) reads it) with `asc` or `desc`, and keeps it in the spec's `sub_order`. The shard checks the path against the sub-aggregations as it does for a `terms` (`aggregation_execution_exception`, 500). A shard keeps every bucket, so the shard result is unchanged; the final reduce, once it has merged the buckets by key, filled the gaps and reduced the sub-aggregations, lists the buckets by the target's value (`InternalOrder.Aggregation`), ties by key ascending (`BucketOrder.compound`'s tie-breaker), a bucket with no value (an `avg` of nothing) last in either direction. The sub-aggregations follow their buckets.

Not in this change, each still refused with "not supported by this node yet": an `order` of several criteria, and a sub-aggregation path through a bucket aggregation (refused as for a `terms`).

## Acceptance criteria

- AC-1: an `order` by a path parses to the path and direction; a path with no element or a bad element fails as it does for a `terms`; `_count`, `_key` and `_term` still parse as before.
- AC-2: over an index of four segments, buckets list by the target's value in either direction, ties by key ascending, with each bucket's sub-aggregations beside it; a stats target is read by its key.
- AC-3: the reduce across shards orders by the reduced values, and gap buckets (no document) sort as the empty value does.
- AC-4: a path naming no sub-aggregation, a stats target without a key, and a path through another bucket aggregation fail the shard as they do for a `terms`.
- AC-5: a `date_histogram` is ordered the same way.

## Test plan

Unit tests in `aggs/bucket/histogram_sub_order_tests.rs` (AC-1, AC-3, AC-4 parse side) and over an `oss-testkit` index (AC-2, AC-4, AC-5). Expected values come from reading `InternalOrder.Aggregation` and `InternalHistogram.reduce` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged: the shard reads the same doc values; the ordering is in the reduce. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: an `order` by a sub-aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. The tie-breaker (key ascending) is the one a `terms` uses; whether `InternalHistogram` compounds the same tie-breaker is read from the Java, not recorded.
