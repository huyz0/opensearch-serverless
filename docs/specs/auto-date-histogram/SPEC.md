# auto_date_histogram aggregation

## Serves

Roadmap R10.2, aggregation gaps: `auto_date_histogram` picks the date interval itself so that a result holds about `buckets` buckets, and answers `interval` with the unit it chose.

## Change

`oss-reader` parses `auto_date_histogram` as `AutoDateHistogramAggregationBuilder.PARSER` does (`field`, `buckets` default 10, `minimum_interval`, `format`, `time_zone`) with the messages Java prints. The shard result is a `date_histogram` at one of the roundings the builder lists (`s` with inner intervals 1, 5, 10, 30; `m` 1, 5, 10, 30; `h` 1, 3, 12; `d` 1, 7; `M` 1, 3; `y` 1, 5, 10, 20, 50, 100), starting at `minimum_interval`: the finest rounding where the buckets stay within `buckets` times the largest inner interval and the data range within `buckets` times that interval's rough duration, as `AutoDateHistogramAggregator.FromSingle` rounds up while it collects. The shard finds it with the engine's `date_histogram` (a min and max pass, then a pass at the candidate rounding, one rounding up while it is too fine), so no engine change is needed. The final reduce is `InternalAutoDateHistogram.reduce`: the coarsest shard rounding, buckets of equal rounded key added, `getAppropriateRounding` re-picking the rounding, empty buckets between the keys, and consecutive buckets merged by the rounding's inner intervals while there are more than `buckets`. The answer is `{"buckets":[{"key_as_string","key","doc_count"}],"interval":"7d"}` (`interval` is the inner interval and the unit letter), `auto_date_histogram#name` with `typed_keys`.

Refused by name: sub-aggregations (merging buckets needs their reduce), `missing`, `script` and `value_type`, and the aggregation on a forwarded shard search.

## Acceptance criteria

- AC-1: a body is read, and refused, with the messages Java 3.10.0 prints for every body recorded (`fixtures/auto-date-histogram/java-auto-date-3.10.0.txt`, 33 bodies).
- AC-2: the final reduce of shard results gives the bytes Java wrote for every recorded case: a day range at targets 3, 10, 29, 30 and 100, shards at different roundings, overlapping buckets, no buckets, 48 hours, `minimum_interval`, an inner interval above 1 (13 recordings).
- AC-3: on a real index the shard picks the rounding the data needs (range and bucket count) and counts each bucket.
- AC-4: with `typed_keys` the result is `auto_date_histogram#name`.
- AC-5: over a REST search the aggregation answers, with `interval`, and a bad body is a 400.
- AC-6: a sub-aggregation is refused by name.

## Test plan

`oss-reader` unit tests (`bucket/auto_date_tests.rs`: AC-1 to AC-4, AC-6) and the node integration test `an_auto_date_histogram_picks_its_interval` (AC-5).

## S3 request budget

None beyond the search's own: the extra passes read the same segments (LIST 0, HEAD 0, PUT 0).

## Switch

None: the aggregation was refused before.

## Risks

Java's range test uses the minimum and maximum of the values as rounded when they were collected, which may be at a finer rounding; here they are the raw extremes rounded at the rounding tested. They differ only at a boundary. The shard makes up to a few more passes over its matches than Java's one.
