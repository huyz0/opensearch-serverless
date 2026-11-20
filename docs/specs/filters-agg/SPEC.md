# Reader: `filters` aggregation

## Serves

Roadmap R10.2: OpenSearch's `filters` bucket aggregation of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `filters` (as `FiltersAggregationBuilder.parse` does at OpenSearch 3.10.0, each filter read as `parseInnerQueryBuilder` reads a query) at the top level of a request's `aggs`, builds each filter as the query it is before the shard opens its point files, runs the filters over the query's live matches with lucene-aggs' `filters` node (the port of `FiltersAggregator`), keeps what `InternalFilters` holds (`aggs::bucket::filters::FiltersResult`), reduces shard results as `InternalFilters.reduce` does (reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it), and writes it as `InternalFilters.doXContentBody` does.

Served keys: `filters` (an array, keyed `0`, `1`, ... in request order, or an object, sorted by key as the builder sorts it), `other_bucket` and `other_bucket_key` (default `_other_`; a key given turns the bucket on unless `other_bucket` is false). Sub-aggregations are refused with "not supported by this node yet". The single-filter `filter` aggregation is [filter-agg](../filter-agg/SPEC.md).

## Acceptance criteria

- AC-1: a `filters` body parses to its filters, in request order for an array and sorted by key (by UTF-16 code unit) for an object; the other bucket is on by `other_bucket: true` or by a key given, off by `other_bucket: false`; an empty or missing `filters` fails with "[filters] cannot be empty."; an unknown key, or a known one of the wrong type, fails as OpenSearch's "Unknown key for a <token> in [<name>]: [<key>]."; a malformed filter fails as a malformed query does.
- AC-2: each filter counts the query's matching documents the filter also matches, a document counting once in every filter it matches; the other bucket, last, counts those matching none.
- AC-3: a query matching nothing gives every bucket 0; a filter on an unmapped field matches nothing; a filter the shard cannot build fails the search with a 400, as the same query would.
- AC-5: shard results reduce as `InternalFilters.reduce`: the counts of each bucket add; shards that disagree on the buckets do not reduce.
- AC-6: the response writes `buckets` as an array of `{"doc_count":n}` or, for an object of filters, an object named by the keys.
- AC-7: a search reserves each `filters`' buckets from the request-body pool with its hits; more buckets (filters and the other one) than the bucket limit (65,535) fail the shard with `too_many_buckets_exception` at 503.
- AC-8: a cold term search with a `filters` aggregation beside its metrics costs the requests and round trips the search alone costs, and a warm one none (the rows `search.metric_aggs.*` of [request-budgets.md](../../request-budgets.md), whose request now holds a `filters`).

## Test plan

Unit tests in `aggs/bucket/filters_tests.rs` (AC-1, AC-5, AC-6), `aggs/bucket/filters_collect_tests.rs` over an `oss-testkit` index (AC-2, AC-3, AC-7), `crates/coordinator/tests/filters_reduce.rs` (AC-5 across shards), the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8). Expected values come from reading `FiltersAggregationBuilder`, `FiltersAggregator` and `InternalFilters` at 3.10.0; no stock recording of these steps exists yet.

## S3 request budget

Unchanged: filters run over the same doc values, postings and points, through the same page cache and ranged GETs as the query (AC-8); a filter that reads points opens them as the query does. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `filters` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. The filters' queries are bounded by the request body's recording, not reserved separately. OpenSearch builds the filters' queries when the shard builds the aggregator, so a bad one fails inside a shard failure; here it fails as a query of the request does.
