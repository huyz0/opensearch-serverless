# Reader: `date_range` aggregation

## Serves

Roadmap R10.2: OpenSearch's `date_range` bucket aggregation of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `date_range` (as `DateRangeAggregationBuilder` does at OpenSearch 3.10.0) and serves it through the `range` machinery of [range-agg](../range-agg/SPEC.md): the same spec, shard node, result, reduce and answer. The only difference is the bounds: a `from` or `to` may be text, a date in the default format (`strict_date_optional_time` or epoch milliseconds, as a `range` query's bound reads it), read as epoch milliseconds, or a number of epoch milliseconds. A date field's buckets already carry `from_as_string`, `to_as_string` and a generated key of the formatted bounds.

Served keys: `field`, `keyed`, `ranges` with `key`, `from` and `to`. Not in this change, each refused with "not supported by this node yet": `format`, `time_zone`, `missing`, `value_type`, `script`, and a bound with date math (`now`, `||`), which needs the request's instant.

## Acceptance criteria

- AC-1: a `date_range` body parses as a `range`'s does, a text bound to epoch milliseconds; a text that is no date fails as a date field's parse fails; date math in a bound and every unserved key above are refused as not supported yet; an unknown key and the other errors name `[date_range]`.
- AC-2: each range counts the matching documents with `from <= value < to`, bounds given as dates or numbers.
- AC-3: the result, reduce and answer are a `range`'s; a `date_range` of a date field writes `from_as_string`, `to_as_string` and the generated key as the `range` does.

## Test plan

Unit tests in `aggs/bucket/date_range_tests.rs` (AC-1) and over an `oss-testkit` index whose long field `n` stands in for a date field (AC-2, AC-3). Expected values come from reading `DateRangeAggregationBuilder` and `RangeAggregator` at 3.10.0; no stock recording exists yet.

## S3 request budget

Unchanged from `range`: LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a `date_range` aggregation was refused before, and one without is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. Both bounds are read without rounding up (a date missing its time starts the day); the Java's rounding of `to` is read from the source, not recorded.
