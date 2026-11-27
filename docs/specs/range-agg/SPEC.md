# Reader: `range` aggregation over a numeric field

## Serves

Roadmap R10.2: OpenSearch's `range` on a long, integer or date field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `range` (as `RangeAggregationBuilder`'s `ObjectParser` and `RangeAggregator.Range.PARSER` do at OpenSearch 3.10.0) at the top level of a request's `aggs`, sorts its ranges as `AbstractRangeBuilder.sortRanges` does, runs it over the query's live matches with lucene-aggs' `range` node (the port of `RangeAggregator`), keeps what `InternalRange` holds (`aggs::bucket::range::RangeResult`), reduces shard results as `InternalRange.reduce` does (`aggs::bucket::range::reduce`, reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it), and writes it as `InternalRange.doXContentBody` does.

Served keys: `field`, `keyed` and `ranges`, each range with `key`, `from` and `to` (a number or null; an absent end is open). Not in this change, each refused with "not supported by this node yet": `missing`, `value_type`, `format`, `script`, and a `from` or `to` given as text. Its sub-aggregations are served as [fixed-bucket-subs](../fixed-bucket-subs/SPEC.md) serves them.

## Acceptance criteria

- AC-1: a `range` body parses to its field, `keyed` (default false) and ranges; the ranges are sorted by `from` then `to`, equal ones in request order, an end that is absent or null being infinite; a missing `field`, a missing or empty `ranges`, an unknown key, a value of the wrong type and every unserved key above fail as the other aggregations' do.
- AC-2: on an index of several segments each range counts the query's matching documents with `from <= value < to`, a document counting once in every range that holds a value of it; ranges may overlap.
- AC-3: an unmapped field, or a query matching nothing, gives every range a count of 0; a keyword or text field fails the shard as OpenSearch fails it.
- AC-4: a date field's buckets carry `from_as_string` and `to_as_string` in the default date format, and a generated key of the formatted bounds.
- AC-5: shard results reduce as `InternalRange.reduce`: the counts of each range add, bucket for bucket; shards that disagree on the ranges do not reduce.
- AC-6: the response writes each bucket as `key`, `from`, `to`, `doc_count` (an open end has no `from` or `to`); the key asked for, else `<from>-<to>` with `*` for an open end and doubles as `Double.toString` writes them; `keyed` writes an object named by the keys.
- AC-7: a search reserves each `range`'s buckets from the request-body pool with its hits; more ranges than the bucket limit (65,535, `search.max_buckets`'s default) fail the shard with `too_many_buckets_exception` at 503.
- AC-8: a cold term search with a `range` aggregation beside its metrics costs the requests and round trips the search alone costs, and a warm one none (the rows `search.metric_aggs.*` of [request-budgets.md](../../request-budgets.md), whose request now holds a `range`).

## Test plan

Unit tests in `aggs/bucket/range_tests.rs` (parsing, reduce, writing: AC-1, AC-4 to AC-6), `aggs/bucket/range_collect_tests.rs` over an `oss-testkit` index (AC-2, AC-3, AC-7), `crates/coordinator/tests/it/range_reduce.rs` (AC-5 across shards), the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8). Expected values come from reading `InternalRange`, `RangeAggregator` and `AbstractRangeBuilder` at 3.10.0; no stock recording of these steps exists yet.

## S3 request budget

Unchanged: doc values are read through the same page cache and ranged GETs as the other aggregations (AC-8). LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a request with a `range` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. OpenSearch checks `ranges` and `field` when the shard builds the aggregator, so its error is wrapped in a shard failure; this node fails them at parse time like `terms`. A `range` with sub-aggregations, text bounds and `format` come later.
