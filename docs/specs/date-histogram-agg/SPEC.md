# Reader: `date_histogram` aggregation over a date field

## Serves

Roadmap R10.2: OpenSearch's `date_histogram` on a date (or long) field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory). The second of the histogram aggregations after [histogram-agg](../histogram-agg/SPEC.md), whose order, bounds and fill it shares.

## Change

`oss-reader` parses `date_histogram` (as `DateHistogramAggregationBuilder`, `DateIntervalWrapper` and `LongBounds` do at OpenSearch 3.10.0) at the top level of a request's `aggs`, runs it over the query's live matches with lucene-aggs' `date_histogram` node (the port of `DateHistogramAggregator` and `Rounding.Prepared`), keeps what `InternalDateHistogram` holds (`aggs::bucket::date_histogram::DateHistogramResult`), reduces shard results as `InternalDateHistogram.reduce` does as a final reduce (`date_histogram::reduce`, reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it), and writes it as `InternalDateHistogram.doXContentBody` does.

Served keys: `field`, `calendar_interval` (`year`, `quarter`, `month`, `week`, `day`, `hour`, `minute`, `second` and their short names), `fixed_interval` (a time value in `nanos`, `micros`, `ms`, `s`, `m`, `h`, `d`), `time_zone` (a fixed offset: `Z`, `UTC`, `GMT`, `UT`, `+hh:mm` and its short forms, alone or after `UTC`, or a number of hours), `offset` (milliseconds or a signed time value), `keyed`, `min_doc_count`, `extended_bounds` and `hard_bounds` (epoch milliseconds or a date in the default format) and `order` (`_count` or `_key`, `asc` or `desc`, one criterion). Not in this change, each refused with "not supported by this node yet": `interval` (the deprecated key), `format`, `missing`, `script`, `value_type`, a time zone with daylight saving (a region id such as `Europe/Paris`), date math in a bound, sub-aggregations, and `order` by a sub-aggregation or with several criteria.

## Acceptance criteria

- AC-1: a body parses to its field, rounding (a calendar unit or a fixed interval in milliseconds, the zone's offset, the offset), keyed (default false), min doc count (default 0), order (default `_key` ascending) and bounds, each bound rounded down without the offset; a calendar interval that is not a unit, a fixed interval that is not a time value (missing unit, a fraction, a negative, zero), both interval keys, no interval, a negative `min_doc_count`, bounds whose min is above their max and extended bounds outside the hard ones fail with OpenSearch's messages; an unknown key and a missing `field` fail as the other aggregations' do; every unserved key above is refused as not supported yet.
- AC-2: on an index of several segments, the buckets are the distinct rounded values of the matches, inside the hard bounds, counted once a document.
- AC-3: an unmapped field or a match-none query gives no buckets; a keyword or text field fails the shard as OpenSearch fails it.
- AC-4: a date field's buckets carry `key_as_string` in the default date format, in the aggregation's zone (`Z` for a zero offset, else `+hh:mm`), before `key`; a long field's buckets carry the key alone; a keyed answer is an object by the key's text.
- AC-5: shard results reduce as `InternalDateHistogram.reduce`: equal keys add, buckets under `min_doc_count` go, with `min_doc_count` 0 the empty buckets between the keys (and out to `extended_bounds`, shifted by the offset) are made by the rounding's next value, then buckets are ordered (key descending reverses; count orders run the same unstable sort OpenSearch runs).
- AC-6: the response writes `buckets` as an array, or an object keyed by the key's text when `keyed`, each bucket `key_as_string` (dates only), `key` (a long), `doc_count`.
- AC-7: a search reserves each `date_histogram`'s memory from the request-body pool with its hits (the bucket limit's worth of 64-byte buckets); a shard whose answer, gaps included, passes the bucket limit (65,535) fails with `too_many_buckets_exception` at 503. The reduce makes no more empty buckets than the limit allows and refuses beyond it.

## Test plan

Unit tests in `aggs/bucket/date_histogram_tests.rs` (parsing, reduce, writing: AC-1, AC-4 to AC-7), `aggs/bucket/date_histogram_collect_tests.rs` over an `oss-testkit` index whose long field `n` stands in for a date field (AC-2, AC-3, AC-7) and `crates/coordinator/tests/date_histogram_reduce.rs` (across shards, AC-5). Expected values come from reading the 3.10.0 Java; no stock recording of these steps exists yet.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as the other aggregations. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `date_histogram` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps, so expected bodies are read from the Java. OpenSearch fails a missing interval, bad bounds and unparsable dates when the shard builds the aggregator, which is a shard failure; this node fails them while parsing the request. Only fixed-offset zones are served: the engine's rounding has no daylight-saving rules. The fill's `too_many_buckets` count and the coordinator's handling of a refused fill are as for `histogram`.
