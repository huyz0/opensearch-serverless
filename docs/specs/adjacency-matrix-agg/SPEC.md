# adjacency_matrix aggregation

## Serves

Roadmap R10.2, aggregation gaps: `adjacency_matrix` counts the documents matching each of several filters and each pair of them, the matrix of which filters overlap.

## Change

`oss-reader` parses `adjacency_matrix` as `AdjacencyMatrixAggregationBuilder` does: `filters` (an object of named queries), `separator` (default `&`) and `show_only_intersecting`, with the messages Java prints. The filters are sorted by key and composed into the engine's `filters` node: each filter and, for each pair `a`, `b` in key order, a `bool must` of both under the key `a<separator>b`, all sorted by key by UTF-16 code unit. A result is written as `{"buckets":[{"key","doc_count"}]}` with only the buckets that hold documents, and `adjacency_matrix#name` with `typed_keys`. Shards that answered add per key through the existing `filters` reduce.

Refused by name: sub-aggregations, more than 100 filters (`index.max_adjacency_matrix_filters` is not read per index), and the aggregation on a forwarded shard search ([forwarded-aggregations](../forwarded-aggregations/SPEC.md) is unchanged).

## Acceptance criteria

- AC-1: a body is read, and refused, with the messages Java 3.10.0 prints for every body recorded (`fixtures/adjacency-agg/java-adjacency-3.10.0.txt`).
- AC-2: an array or null `filters`, an unknown field, a wrong value type and a missing `filters` fail as Java fails them.
- AC-3: each filter and each pair of filters counts the documents it matches on a real index, keys sorted.
- AC-4: `separator` names the pairs; `show_only_intersecting` leaves out the single filters.
- AC-5: only buckets holding documents are written, each with its key.
- AC-6: over a REST search the aggregation answers, typed or not, and a bad body is a 400.
- AC-7: a sub-aggregation is refused by name.

## Test plan

`oss-reader` unit tests (`bucket/adjacency_tests.rs`: AC-1 to AC-5, AC-7) and the node integration test `an_adjacency_matrix_counts_each_filter_and_each_pair` (AC-6).

## S3 request budget

None beyond the search's own: LIST 0, HEAD 0, PUT 0.

## Switch

None: the aggregation was refused before.

## Risks

A pair costs one more query per shard, up to 4,950 for 100 filters; the bucket limit bounds the buckets. A missing `filters` is Java's `IllegalStateException`, answered here with status 400 (Java answers 500).
