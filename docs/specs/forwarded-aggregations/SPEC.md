# Forwarded aggregations: single-value metrics

## Serves

Roadmap R10.2 and R7: a Java coordinator's last-resort ask (`internal:serverless/search/shard`) for a shard a Rust node writes used to refuse any request with aggregations ("a forwarded search with [aggregations] is not supported by this node"), so a plugin that aggregates over such an index (anomaly detection) failed with a 503. This slice serves `min`, `max`, `sum`, `avg` and `value_count` over one field. See [ADR 0057](../../adr/0057-a-rust-owner-answers-the-java-search-shard-action.md) for the ask.

## Change

`oss-transport` reads the source's `AggregatorFactories.Builder` (`SearchSourceBuilder.aggregations`) as the Java shell writes it: a count, then each aggregation as its writeable name (`min`, `max`, `sum`, `avg`, `value_count`), its name, an empty sub-builder, a null metadata value, `value_count`'s null target value type, the field, and no script, value type, format, `missing` or time zone, then an empty pipeline list. It writes the shard's reply's aggregations as `InternalAggregations.writeTo` does: each result as its type name, its name, null metadata, `raw` (`DocValueFormat.RAW`, which writes nothing more) and its value (`InternalMax`, `InternalMin` and `InternalSum`: a double; `InternalAvg`: a double and a count; `InternalValueCount`: a count). `oss-node` turns the request's aggregations into the `aggs` of the search body the reader runs, and the reader's shard-level results into the reply.

Refused by name, as before: a sub-aggregation, a pipeline aggregation, metadata, a script, a format, a `missing` value, a time zone, any other aggregation type, and any result of another kind.

## Acceptance criteria

- AC-1: a request with these aggregations reads, and a request written from the read one reads back the same.
- AC-2: another aggregation type, a sub-aggregation, and a pipeline aggregation are refused by name, and nothing after them is read.
- AC-3: the reply's aggregations are `InternalAggregations`' bytes for these six results.
- AC-4: the handler runs the request's aggregations on the shard and answers them; a result of another kind is refused as a bad request.
- AC-5: an aggregation over a date field is answered with `DocValueFormat.DateTime` of the default date format, in UTC and milliseconds (`date_time`, `strict_date_time_no_millis||strict_date_optional_time||epoch_millis`, print pattern `strict_date_optional_time`, `Z`, resolution 0), where a number's is `raw`. A `max` of the time field is what anomaly detection asks first.
- AC-6: a `stats` aggregation is read and written like the single-value metrics, and answered as `InternalStats`: the format, a `vlong` count, then min, max and sum as doubles.
- AC-7: the request and the answers above are the bytes OpenSearch 3.10.0's own classes write (`fixtures/search-shard/java-aggs-3.10.0.txt`, from `AggRecordings.java`).
- AC-8: a `cardinality` aggregation, with its `precision_threshold` if any, is read, written and answered as `InternalCardinality` (the format, a present flag, and the sketch `AbstractHyperLogLogPlusPlus.writeTo` writes, or the absent flag when no value was seen); an `execution_hint` is refused by name. The sketch's bytes written from a read one are the ones Java wrote, for no hashes, a few, and a HyperLogLog one.
- AC-9: a `percentiles` or `percentile_ranks` aggregation, with its keys, `keyed` and a t-digest `compression` if any, is read, written and answered as `InternalTDigestPercentiles` or `InternalTDigestPercentileRanks` (`tdigest_percentiles`, `tdigest_percentile_ranks`): the format, the keys, the digest as `TDigestState.write` writes it from 3.4.0 (the verbose `MergingDigest` encoding), and `keyed`; a shard with no value answers an empty digest of the asked compression. Another method than the t-digest is refused by name. The digest bytes of a Rust digest are the bytes Java wrote.

## Test plan

Unit tests in `oss-transport` (`aggs.rs`: AC-1 to AC-3 and AC-5), the Java recordings in `crates/transport/tests/search_shard_aggs.rs` (AC-1, AC-3, AC-5 to AC-7) and `oss-node` (`search_shard_action/tests.rs`, `convert_tests.rs`: AC-4, AC-6). `AggRecordings.java` records the bytes of the 3.10.0 classes (`AggregatorFactories.Builder`, `InternalAggregations`); the first two slices were written from reading the Java source and the recordings agree with them.

## S3 request budget

None of its own: the shard search runs the aggregations as a REST search of the same index does. LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before; a request without aggregations is unchanged.

## Risks

Written from the Java source, not from Java's bytes. A mismatch fails the coordinator's read of the reply, not the shard.
