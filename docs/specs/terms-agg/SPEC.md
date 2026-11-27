# Reader: `terms` aggregation over a numeric field

## Serves

Roadmap R10.2, first bucket aggregation: OpenSearch's `terms` on a long, integer or date field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `terms` (as `TermsAggregationBuilder`'s `ObjectParser` does at OpenSearch 3.10.0) at the top level of a request's `aggs`, runs it over the query's live matches with lucene-aggs' numeric `terms` (the port of `NumericTermsAggregator`), keeps what `LongTerms` holds (`aggs::bucket::terms::TermsResult`), reduces shard results as `InternalTerms.reduce` does (`aggs::bucket::terms::reduce`, reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it), and writes it as `LongTerms.doXContentBody` does. A single-shard search answers the reduce of its one result, as OpenSearch does.

Served keys: `field`, `size`, `shard_size`, `min_doc_count` and `order` (`_count` or `_key`, `asc` or `desc`, one criterion). Not in this change, each refused with "not supported by this node yet": fields of other non-numeric types (keyword is [keyword-terms-agg](../keyword-terms-agg/SPEC.md)), `include`, `exclude`, `missing`, `script`, `value_type`, `format`, `execution_hint`, `collect_mode` (`show_term_doc_count_error` is [terms-doc-count-error](../terms-doc-count-error/SPEC.md)) and `order` by a sub-aggregation or with several criteria. Sub-aggregations are [numeric-terms-subs](../numeric-terms-subs/SPEC.md). Other bucket types (`histogram`, `date_histogram`, `range`, `filter`, `filters`) are their own changes.

## Acceptance criteria

- AC-1: a `terms` body parses to its field, size (default 10), shard size (default `size * 1.5 + 10`, never below `size`), min doc count (default 1) and order (default `_count` descending, ties by key ascending); `size` and `shard_size` below 1 and a negative `min_doc_count` fail with OpenSearch's messages; an unknown key and a missing `field` fail as the other aggregations' do; every unserved key above is refused as not supported yet.
- AC-2: on an index of several segments, the buckets of a long field are its distinct values with their document counts over the query's matches, ordered by count descending and key ascending, cut at `size`, with `sum_other_doc_count` the documents of the buckets cut; `order` by `_key` and by `_count` ascending list as OpenSearch orders them; a document with several values counts once under each.
- AC-3: `min_doc_count` above 1 drops the buckets under it and counts their documents in `sum_other_doc_count`; an unmapped field gives no buckets; a match-none query gives no buckets.
- AC-4: a date field's buckets carry `key_as_string` in the default date format after `key`.
- AC-5: shard results reduce as `InternalTerms.reduce`: equal keys add their counts, the merged buckets are filtered by `min_doc_count`, ordered, cut at `size` with the cut documents added to `sum_other_doc_count`, and `doc_count_error_upper_bound` is 0 for one shard, otherwise the sum of what each full shard may have cut (the count of its last bucket under `_count` descending, 0 when it returned fewer than `shard_size` buckets or is ordered by key, -1 under `_count` ascending).
- AC-6: the response writes `doc_count_error_upper_bound`, `sum_other_doc_count` and `buckets` in that order, each bucket `key`, optionally `key_as_string`, `doc_count`, in OpenSearch's name order among the other aggregations.
- AC-7: a search reserves each `terms`' memory from the request-body pool with its hits: `shard_size` buckets of 16 bytes (at most the bucket limit) and 64 bytes for each document of the shard, the most distinct values it can count while it collects; a shard keeping more buckets than the bucket limit (65,535, `search.max_buckets`'s default) fails with `too_many_buckets_exception` at 503, as OpenSearch answers it.
- AC-8: a cold term search with a `terms` aggregation beside its metrics costs the requests and round trips the search alone costs, and a warm one none (the rows `search.metric_aggs.*` of [request-budgets.md](../../request-budgets.md), whose request now holds a `terms`).

## Test plan

Unit tests in `aggs/bucket/terms_tests.rs` (parsing, reduce, writing: AC-1, AC-5, AC-6, AC-4), `aggs/bucket/collect_tests.rs` over an `oss-testkit` index (AC-2, AC-3, AC-7), `crates/coordinator/tests/it/terms_reduce.rs` (a `terms` across shards, AC-5), the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8). Expected values come from reading `InternalTerms`, `LongTerms` and `NumericTermsAggregator` at 3.10.0; no stock recording of these steps exists yet (the compat replay is a follow-up, see Risks).

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as the metric aggregations (AC-8). LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `terms` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps yet, so the expected bodies are read from the Java, not observed: the first recorded `terms` step in `fixtures/compat/search/` may show a difference. The reservation counts one distinct value a document, so a field with several values a document may count more than it reserved; lucene-aggs offers no bound on distinct values while collecting. On a shard of many segments OpenSearch may collect in concurrent slices, which this node does not.
