# Reader: `terms` ordered by a path through single-bucket aggregations

## Serves

Roadmap R10.2 (aggregations): a `terms`, `histogram` or `date_histogram` ordered by a value below a `filter`, `missing`, `nested` or `reverse_nested` sub-aggregation, which dashboards use to rank terms by a figure in a nested or filtered slice. I-13 (REST matches OpenSearch's recorded responses).

## Change

The `order` path of a bucket aggregation is walked as `AggregationPath` and `Aggregator.resolveSortPathOnValidAgg` walk it at OpenSearch 3.10.0: any number of single-bucket aggregations (`filter`, `missing`, `nested`, `reverse_nested`, `global`) joined by `>`, then a metric (with its key, as for a metric directly under the `terms`) or a single-bucket aggregation, which sorts by its `doc_count` (`f`, `f.doc_count`). Each bucket sorts by the value found there, NaN last and ties by term, at the shard (before the cut at `shard_size`) and at the reduce (on the reduced value).

A path the shard refuses fails as stock fails, as an `aggregation_execution_exception` carrying an `illegal_argument_exception` cause with the reason alone: an element that does not exist, a key that is not on the last element, a path through a non-single-bucket aggregation, a metric key stock rejects, a key other than `doc_count` on a single-bucket aggregation.

## Acceptance criteria

- AC-1: over numeric `terms`, an order by a `filter`'s sum, stats key, document count (`f` and `f.doc_count`), a `missing`'s sum and a `filter` in a `filter` equals the stock recording (`fixtures/compat/nested-search/terms-sub-order.json`).
- AC-2: over a keyword `terms`, an order by a `nested` aggregation's max, avg (with `size` 2, so `sum_other_doc_count` counts the cut) and document count equals the recording; a `terms` under `nested` ordered by a `reverse_nested`'s sum and document count equals it.
- AC-3: a path with a key on a non-last element, through a `terms` or a metric, to an unknown name, to a multi-value metric without a key, with a bad key on a metric or on a single-bucket aggregation, or with `_count` as the key fails as the recording does (status, bodies, cause chain).
- AC-4: the sort value of a path, and the check of one, over `Value::Filters` results (unit tests).
- AC-5: the stock answers for a `filter` or `missing` under a keyword `terms` and for an order of several criteria, which this node refuses today, are recorded (`fixtures/compat/search-metrics-gaps/terms-bucket-subs.json`, not replayed).

## Test plan

The recording is replayed through an assembled node with nested enabled (`an_assembled_node_answers_every_nested_fixture_as_stock_opensearch_did`, crates/compat/tests/node_cluster.rs); unit tests in `aggs/bucket/sub_order_path_tests.rs`. Recorded from stock 3.10.0 (release tarball, SHA-512 checked) on 2026-11-27.

## S3 request budget

Unchanged: the order reads values the aggregation already collected. LIST 0, HEAD 0, PUT 0.

## Switch

None: the paths were refused or mis-sorted before; a request without one is unchanged.

## Risks

One shard and six documents are recorded, so the reduce of several shards' single-bucket values is held by the unit tests, not by stock. A `filter` or `missing` under a keyword `terms` is still refused, so the keyword cases that replay use `nested`.
