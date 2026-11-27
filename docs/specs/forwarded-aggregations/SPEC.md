# Forwarded aggregations: single-value metrics

## Serves

Roadmap R10.2 and R7: a Java coordinator's last-resort ask (`internal:serverless/search/shard`) for a shard a Rust node writes used to refuse any request with aggregations ("a forwarded search with [aggregations] is not supported by this node"), so a plugin that aggregates over such an index (anomaly detection) failed with a 503. This slice serves `min`, `max`, `sum`, `avg` and `value_count` over one field. See [ADR 0057](../../adr/0057-a-rust-owner-answers-the-java-search-shard-action.md) for the ask.

## Change

`oss-transport` reads the source's `AggregatorFactories.Builder` (`SearchSourceBuilder.aggregations`) as the Java shell writes it: a count, then each aggregation as its writeable name (`min`, `max`, `sum`, `avg`, `value_count`), its name, an empty sub-builder, a null metadata value, `value_count`'s null target value type, the field, and no script, value type, format, `missing` or time zone, then an empty pipeline list. It writes the shard's reply's aggregations as `InternalAggregations.writeTo` does: each result as its type name, its name, null metadata, `raw` (`DocValueFormat.RAW`, which writes nothing more) and its value (`InternalMax`, `InternalMin` and `InternalSum`: a double; `InternalAvg`: a double and a count; `InternalValueCount`: a count). `oss-node` turns the request's aggregations into the `aggs` of the search body the reader runs, and the reader's shard-level results into the reply.

Refused by name, as before: a sub-aggregation, a pipeline aggregation, metadata, a script, a format, a `missing` value, a time zone, any other aggregation type, an aggregation over a date field (its format is not `raw`), and any result of another kind.

## Acceptance criteria

- AC-1: a request with these aggregations reads, and a request written from the read one reads back the same.
- AC-2: another aggregation type, a sub-aggregation, and a pipeline aggregation are refused by name, and nothing after them is read.
- AC-3: the reply's aggregations are `InternalAggregations`' bytes for these five results.
- AC-4: the handler runs the request's aggregations on the shard and answers them; an aggregation over a date field, or of another kind, is refused as a bad request.

## Test plan

Unit tests in `oss-transport` (`aggs.rs`: AC-1 to AC-3) and `oss-node` (`search_shard_action/tests.rs`, `convert_tests.rs`: AC-4). The expected bytes are read from the 3.10.0 Java (`AggregatorFactories.Builder`, `ValuesSourceAggregationBuilder.doWriteTo`, `InternalAggregations.writeTo`, `InternalMax`, `InternalAvg`, `InternalValueCount`). No Java-written fixture exists for them yet: `fixtures/search-shard/WriteSearchShard.java` would need entries and a run.

## S3 request budget

None of its own: the shard search runs the aggregations as a REST search of the same index does. LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before; a request without aggregations is unchanged.

## Risks

Written from the Java source, not from Java's bytes. A mismatch fails the coordinator's read of the reply, not the shard.
