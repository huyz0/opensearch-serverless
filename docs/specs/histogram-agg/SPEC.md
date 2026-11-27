# Reader: `histogram` aggregation over a numeric field

## Serves

Roadmap R10.2, second bucket aggregation: OpenSearch's `histogram` on a long, integer or date field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `histogram` (as `HistogramAggregationBuilder`'s `ObjectParser` and `DoubleBounds` do at OpenSearch 3.10.0) at the top level of a request's `aggs`, runs it over the query's live matches with lucene-aggs' `histogram` node (the port of `NumericHistogramAggregator`), keeps what `InternalHistogram` holds (`aggs::bucket::histogram::HistogramResult`), reduces shard results as `InternalHistogram.reduce` does as a final reduce (`aggs::bucket::histogram::reduce`, reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it), and writes it as `InternalHistogram.doXContentBody` does. A single-shard search answers the reduce of its one result, as OpenSearch does.

Served keys: `field`, `interval`, `offset`, `keyed`, `min_doc_count`, `extended_bounds`, `hard_bounds` and `order` (`_count` or `_key`, `asc` or `desc`, one criterion). Not in this change, each refused with "not supported by this node yet": `missing`, `value_type`, `format`, `script` and `order` by a sub-aggregation or with several criteria. Its sub-aggregations are served as [keyed-bucket-subs](../keyed-bucket-subs/SPEC.md) serves them. `date_histogram` is its own change.

## Acceptance criteria

- AC-1: a `histogram` body parses to its field, interval, offset (default 0), keyed (default false), min doc count (default 0), order (default `_key` ascending) and bounds; `interval` of 0 or below, a negative `min_doc_count`, a bound that is not finite or whose max is below its min, and extended bounds outside the hard ones fail with OpenSearch's messages; an unknown key and a missing `field` fail as the other aggregations' do; every unserved key above is refused as not supported yet.
- AC-2: on an index of several segments, the buckets are the distinct `floor((v - offset) / interval)` of the matches' values, each keyed `n * interval + offset` and counted once a document, inside the hard bounds; a bucket is made only for a value that exists.
- AC-3: an unmapped field or a match-none query gives no buckets; a zero or missing interval fails the shard with OpenSearch's message.
- AC-4: a date field's buckets carry `key_as_string` in the default date format before `key`; a keyed answer is an object by the key's text.
- AC-5: shard results reduce as `InternalHistogram.reduce`: equal keys (`Double.compare`) add, buckets under `min_doc_count` go, with `min_doc_count` 0 the empty buckets between the keys (and out to `extended_bounds`) are made by `round(key + 1.5 * interval)`, then buckets are ordered (key descending reverses; count orders run the same unstable sort OpenSearch runs).
- AC-6: the response writes `buckets` as an array, or an object keyed by the key's text when `keyed`, each bucket `key_as_string` (dates only), `key` (a double as Java prints it), `doc_count`.
- AC-7: a search reserves each `histogram`'s memory from the request-body pool with its hits (the bucket limit's worth of 64-byte buckets); a shard whose answer, gaps included, passes the bucket limit (65,535, `search.max_buckets`'s default) fails with `too_many_buckets_exception` at 503, as OpenSearch answers it. The reduce makes no more empty buckets than the limit allows and refuses beyond it.

## Test plan

Unit tests in `aggs/bucket/histogram_tests.rs` (parsing, reduce, writing, and runs on an `oss-testkit` index: AC-1 to AC-7) and `crates/coordinator/tests/it/histogram_reduce.rs` (a `histogram` across shards, AC-5). Expected values come from reading `InternalHistogram`, `HistogramAggregationBuilder` and `NumericHistogramAggregator` at 3.10.0; no stock recording of these steps exists yet.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as the other aggregations. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `histogram` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps yet, so expected bodies are read from the Java. OpenSearch counts the gaps of a final reduce only after making them; this node stops at the limit, so the count in a `too_many_buckets_exception` from a fill reads one past the limit where OpenSearch reads the full count. `partial::reduce` answers `None` for a fill past the limit, which `reduce_shards` reports as a mismatch until the coordinator's search path (R7) maps it to the 503. A date field read by `histogram` is read as its milliseconds.
