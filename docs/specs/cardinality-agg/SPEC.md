# Reader: the cardinality aggregation

## Serves

Roadmap R10.3, the reader half, continued from [metric-aggs](../metric-aggs/SPEC.md): `cardinality` on a reader's single-shard `_search`, with a shard-level sketch a coordinator can reduce (I-13, REST matches OpenSearch's recordings; ADR 0004, bounded memory).

## Change

`oss-reader` parses `cardinality` as `CardinalityAggregationBuilder`'s `ObjectParser` does at OpenSearch 3.10.0 (`field`, `precision_threshold` coerced as Jackson and `Numbers.toLong` coerce it, `execution_hint`, `rehash` read and ignored, no `format`), and serves it on integer, long and date fields at the top level. lucene-aggs builds the `HyperLogLogPlusPlus` sketch of the matches' `mix64` hashes and writes it as `AbstractHyperLogLogPlusPlus.writeTo` does; `aggs::metric::hll` reads it, estimates it (`AbstractLinearCounting` and `AbstractHyperLogLog.cardinality` with OpenSearch's bias tables) and merges it (`HyperLogLogPlusPlus.merge`, for `InternalCardinality.reduce`). Every metric of a request is now computed in one `aggregate_tree` pass. The shard fails as OpenSearch's does for an unknown `execution_hint` and for a threshold whose precision overflows, in OpenSearch's order.

Not in this change: `cardinality` of a keyword field ([keyword-cardinality](../keyword-cardinality/SPEC.md) serves it), `missing`, `script`, `value_type`, and the coordinator's reduce (R7). Each is refused with "not supported by this node yet".

## Acceptance criteria

- AC-1: the recorded `cardinality` searches (`fixtures/compat/search/paging-sort.json`: integer and date fields, beside another metric, with precision thresholds that count linearly and that pass to HyperLogLog, a query's matches, an unmapped field) get the status and body OpenSearch 3.10.0 gave, replayed against a reader node.
- AC-2: the recorded `cardinality` errors (`fixtures/compat/search/errors.json`: negative, non-numeric, boolean and overflowing thresholds, unknown and numeric execution hints, `format`, a text field) get OpenSearch's status and body.
- AC-3: for every precision 4 to 18 and 20 counts from 1 to 300,000, the estimate of a sketch of `mix64(0..n)`, and of two sketches merged into an empty one, equals what OpenSearch 3.10.0's `HyperLogLogPlusPlus` printed (300 cases).
- AC-4: `precisionFromThreshold` gives OpenSearch's precision, or its `maxValue must be non-negative (got: x)`, for thresholds from 0 to `Long.MAX_VALUE`.
- AC-5: on an index of four segments with 200 distinct values, `cardinality` is 200 at the default precision, 206 at threshold 100 and 168 at threshold 0, as OpenSearch estimates those hashes.
- AC-6: a search reserves each `cardinality`'s sketch, `5 * 2^p + 16` bytes, with its hits.

## Test plan

`crates/compat/tests/node_search.rs` replays AC-1 and AC-2. `aggs/metric/hll_tests.rs` holds AC-3 and AC-4 (vectors printed by OpenSearch's own classes), `aggs/metric/cardinality_tests.rs` the parsing, writing and reduce, `aggs/metric/collect_tests.rs` AC-5 and AC-6.

## S3 request budget

Unchanged: the sketch reads the same doc values as the other metrics, in the same pass. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with `cardinality` was refused before.

## Risks

A sketch's estimate depends on every hash and register rule; a slip shows in AC-3's vectors. A concurrent-sliced OpenSearch shard merges slice sketches, which gives the same registers and the same estimate.
