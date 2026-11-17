# Reader: metric aggregations

## Serves

Roadmap R10.3, the reader half: OpenSearch's metric aggregations on a reader's single-shard `_search`, with shard-level results a coordinator can reduce (I-13, REST matches OpenSearch's recordings; ADR 0004, bounded memory).

## Change

`oss-reader` parses a body's `aggs` (or `aggregations`) as `AggregatorFactories.parseAggregators` does, at OpenSearch 3.10.0, and serves `min`, `max`, `sum`, `avg`, `stats` and `value_count` of integer, long and date fields at the top level: computed over the query's live matches by lucene-aggs (lucene-rust's port of the metric aggregators, compensated sums in Java's order), kept as each `Internal*` aggregation's state (`aggs::metric::partial`), and written after the hits in the order OpenSearch's reduce lists them. `partial::reduce` is each `Internal*` reduce, for the coordinator to call once it exists.

Not in this change: `cardinality`, `percentiles`, `percentile_ranks` and `top_hits` (each its own change), every other aggregation type, sub-aggregations, `missing`, `format`, `script`, `value_type`, `meta`, `value_count` of a keyword, and the coordinator's reduce across shards (R7). Each is refused with "not supported by this node yet"; the differences are in [compatibility.md](../../compatibility.md).

## Acceptance criteria

- AC-1: the recorded `_search` steps with metric aggregations (`fixtures/compat/search/paging-sort.json`: integer and date fields over two segments, a query's matches, no match, an unmapped field, with sort and paging) get the status and body OpenSearch 3.10.0 gave, replayed against a reader node.
- AC-2: the recorded aggregation errors (`fixtures/compat/search/errors.json`: unknown type with its suggestion and cause, missing definition, two types, invalid name, a non-object definition, no field, an unknown key, a non-string field, sub-aggregations under a metric with its 500, and min, stats and value_count of keyword and text fields failing the shard) get OpenSearch's status and body.
- AC-3: on an index of four segments and 200 documents, each metric equals the value computed from the documents, over every match and over a query's matches.
- AC-4: the response lists aggregations in the order a `java.util.HashMap` keyed by their names iterates, as recorded from OpenSearch (put `c, b, a, sum_y, vc`, listed `a, b, c, vc, sum_y`).
- AC-5: shard results reduce as OpenSearch's `Internal*` reduce: min of mins, max of maxes, counts added, sums added with `CompensatedSum` (1e16 + 1 + 1 keeps both ones; inf - inf is Java's NaN).
- AC-6: a cold term search with a `stats` and a `max` aggregation costs the requests and round trips the search alone costs, and a warm one none (rows `search.metric_aggs.*` of [request-budgets.md](../../request-budgets.md)).
- AC-7: a search reserves its aggregations' state from the request-body pool with its hits (`hits_bytes` adds `state_bytes`, which grows with each aggregation and its names).
- AC-8: doubles are written as Java 21's `Double.toString` writes them and dates as the default format prints them, checked against values Java printed.

## Test plan

`crates/compat/tests/node_search.rs` replays AC-1 and AC-2 (red on three steps before the unknown type's cause, the text field's error and a key-order artifact were fixed). Unit tests in `aggs/metric/tests.rs` (parsing, writing, reduce, order), `aggs/metric/collect_tests.rs` (AC-3, AC-7 over an `oss-testkit` index), `double.rs`, `date_text.rs` and `hash_order.rs` (AC-8, AC-4, expected values printed by Java 21); the budget gate in `crates/node/tests/it/aggs_budgets.rs`.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as the rest of a segment, and on the fixture's segments no request is added (AC-6). LIST 0, HEAD 0, PUT 0; an idle index still makes none.

## Switch

None: a request with aggregations was refused before, and one without is unchanged.

## Risks

A Java rule not ported (a resize rule of `HashMap`, a double's formatting) would show as a mismatched recording; the order of more than eight names sharing their low hash bits in a table of 64 or more is not modelled. On a shard of many segments OpenSearch may split the work into concurrent slices and its sums can differ in their last bits; this node answers as OpenSearch does without slicing.
