# Reader: `filter` aggregation

## Serves

Roadmap R10.2: OpenSearch's single `filter` bucket aggregation of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `filter` (as `FilterAggregationBuilder.parse` does at OpenSearch 3.10.0: its body is one query, read as `parseInnerQueryBuilder` reads it) and serves it through the `filters` machinery of [filters-agg](../filters-agg/SPEC.md) as one filter keyed `0` (`FiltersSpec.single`). The shard result keeps what `InternalFilter` holds (one count), reduces as `InternalFilter.reduce` does (the count adds; a single and a multiple result do not reduce together), and is written as `InternalFilter.doXContentBody` does: `"doc_count":n` beside the aggregation's name, with no `buckets`. Its sub-aggregations are served as [fixed-bucket-subs](../fixed-bucket-subs/SPEC.md) serves them.

## Acceptance criteria

- AC-1: a `filter` body parses to one query under the key `0`; a malformed query fails as a malformed query does; sub-aggregations of a `filter` are refused.
- AC-2: the filter counts the query's matching documents it also matches.
- AC-5: shard results reduce by adding the count; a single result and a `filters` result do not reduce together.
- AC-6: the response writes `"doc_count":n`.

## Test plan

Unit tests in `aggs/bucket/filters_tests.rs` (AC-1, AC-5, AC-6) and `aggs/bucket/filters_collect_tests.rs` over an `oss-testkit` index (AC-2). Expected values come from reading `FilterAggregationBuilder` and `InternalFilter` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged from `filters` (the same doc values, postings and points as the query): LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `filter` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps.
