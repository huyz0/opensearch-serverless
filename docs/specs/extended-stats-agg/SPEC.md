# Reader: `extended_stats` aggregation

## Serves

Roadmap R10.2 (aggregations): `extended_stats`, which dashboards use for spread, on the same values `stats` reads. I-13 (REST matches OpenSearch's recorded responses).

## Change

`oss-reader` parses, runs, reduces and writes `extended_stats` as `ExtendedStatsAggregationBuilder`, `ExtendedStatsAggregator` and `InternalExtendedStats` do at OpenSearch 3.10.0: count, min, max, avg, sum, the compensated sum of squares, the variance (population, sampling), the standard deviation (population, sampling) and the deviation bounds at `sigma` (default 2). The shard state is lucene-aggs' `NEED_SQ` state, built value by value as the Java aggregator builds it; the reduce adds the shards' compensated sums in shard order.

Served keys: `field`, `sigma` (a number or numeric text, at least 0); `script` is refused. As `stats` does, `missing`, `value_type` and `format` are refused with "not supported by this node yet". On a date field every value is also written as text (`*_as_string`), and in the bounds object (`std_deviation_bounds_as_string`).

## Acceptance criteria

- AC-1: over numeric fields the answer equals what stock 3.10.0 recorded (`fixtures/compat/search-metrics/extended-stats.json`): doubles, integers, `sigma` 0, 3 and text, no match, one document, equal values, an unmapped field, under `terms`, beside `stats`, `typed_keys`.
- AC-2: on a date field the answer, with every `*_as_string`, equals the recording.
- AC-3: a negative `sigma`, an unknown key, no field, a text field and a keyword field fail as stock fails, with the recorded bodies.
- AC-4: `missing`, `format` and `value_type` are refused as not supported yet, and the stock answers for `missing` and `format` are recorded (`search-metrics-gaps/extended-stats.json`, not replayed).
- AC-5: reducing the states of several shards gives the sum of squares and the count of one shard holding all the values, to within the compensated sum's own error (unit test).

## Test plan

The recorded fixture is replayed through an assembled node (`an_assembled_node_answers_every_metric_fixture_as_stock_opensearch_did`, crates/compat/tests/node_cluster.rs); unit tests in `aggs/metric/extended_stats_tests.rs` cover the parse, the reduce and the writer. Recorded from stock 3.10.0 (release tarball, SHA-512 checked) on 2026-11-27.

## S3 request budget

Unchanged: the aggregation reads doc values of segments the search already reads. LIST 0, HEAD 0, PUT 0.

## Switch

None: it was refused before; a request without it is unchanged.

## Risks

One shard and nine documents are recorded: the reduce across shards is held by the unit test, not by stock. The forwarded shard wire does not carry `extended_stats` (a Java coordinator's request for it is refused by name there).
