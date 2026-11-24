# Reader: a numeric `terms` ordered by a metric sub-aggregation

## Serves

Roadmap R10.2, after the keyword variant in [keyword-terms-agg](../keyword-terms-agg/SPEC.md) (its fifth slice) and [numeric-terms-subs](../numeric-terms-subs/SPEC.md): I-13 (REST matches OpenSearch), ADR 0004 (bounded memory).

## Change

A `terms` over a long or date field accepts `order` by a metric sub-aggregation (`{"m": "desc"}`, `{"m.value": "asc"}`, `{"st.max": "asc"}`), one criterion, as a keyword `terms` does: the key ascending breaks ties (`BucketOrder.compound`'s tie-breaker). A shard cannot know the best buckets before it has counted them all, so the engine keeps every bucket (at most the bucket limit) and the shard orders them by the sub-aggregation's value and cuts to `shard_size` (`BucketSelectionStrategy`), the cut ones' documents joining `sum_other_doc_count`. The reduce orders the merged buckets by the reduced value of each, then cuts at `size`. A metric with no value is NaN and sorts last whichever the direction. The path is checked as `AggregationPath` and the aggregators check it (`aggregation_execution_exception`, 500). This holds at the top level and under another bucket aggregation.

Still refused as not supported yet: an order of several criteria, and a path through other aggregations.

## Acceptance criteria

- AC-1: a shard orders its buckets by the targeted metric, keeps `shard_size` and counts the cut buckets' documents as other, for the `value` key and for a `stats` key, ascending and descending.
- AC-2: shard results reduce by the reduced value of each key, so a bucket cut on one shard but kept on another is ranked by all its shards' values.
- AC-3: equal values list by key ascending, in either direction.
- AC-4: an order path that names no sub-aggregation fails with `aggregation_execution_exception`.

## Test plan

Unit tests in `aggs/bucket/terms_subs_tests.rs`.

## S3 request budget

Unchanged: LIST 0, HEAD 0, PUT 0. The shard keeps up to the bucket limit of buckets in memory, already reserved for the field's values (numeric-terms-subs AC-7).

## Switch

None: a request with this order was refused before.

## Risks

Not yet recorded against a stock node.
