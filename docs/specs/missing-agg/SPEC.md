# Reader: `missing` aggregation

## Serves

Roadmap R10.2: OpenSearch's `missing` bucket aggregation of a single-shard `_search`: one bucket counting the documents with no value for a field, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `missing` (as `MissingAggregationBuilder` does at OpenSearch 3.10.0 through `ValuesSourceAggregationBuilder.declareFields`) and serves it through the `filter` machinery of [filter-agg](../filter-agg/SPEC.md): the bucket is the documents the query `bool { must_not: [ exists { field } ] }` matches, so a field the mapping does not know counts every match, as `MissingAggregator` counts a field without a values source. The result, its reduce and its answer are those of `filter` (`InternalMissing` holds one count and writes `"doc_count":n`). Its sub-aggregations are served as [fixed-bucket-subs](../fixed-bucket-subs/SPEC.md) serves them.

Served key: `field`. Every other key `missing` declares (`missing`, `value_type`, `format`, `script`) is refused with "not supported by this node yet"; an unknown key fails as the other aggregations' do.

## Acceptance criteria

- AC-1: a body `{"field": f}` parses to one query under the key `0` that is `bool` with one `must_not` clause `exists` of `f`; a body with no `field`, a non-string `field`, an unknown key and each unserved key fail as `range`'s do.
- AC-2: the bucket counts the matches that have no value for the field, across the segments of an index; a mapped field nobody has a value for and an unmapped field count every match; a match-none query counts nothing.
- AC-3: shard results reduce and write as a `filter`'s (the count adds; the answer is `"doc_count":n`).
- AC-4: sub-aggregations run under it as under `filter`, over the missing documents only.

## Test plan

Unit tests in `aggs/bucket/missing_tests.rs` (AC-1, AC-3) and `aggs/bucket/missing_collect_tests.rs` over an `oss-testkit` index (AC-2, AC-4). Expected values come from reading `MissingAggregationBuilder` and `MissingAggregator` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged from `filter` (the same doc values as the `exists` query): LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `missing` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. `exists` on a text field reads norms or terms as the `exists` query does, which is what `missing` counts on in OpenSearch too (`MissingAggregator` reads the field's doc values and so counts an unindexed doc-values-less field's documents as missing; this node follows the `exists` query's rule).
