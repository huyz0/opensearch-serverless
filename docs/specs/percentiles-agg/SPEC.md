# Reader: the percentiles and percentile_ranks aggregations

## Serves

Roadmap R10.3, the reader half, continued from [cardinality-agg](../cardinality-agg/SPEC.md): `percentiles` and `percentile_ranks` on a reader's single-shard `_search`, with a shard-level digest a coordinator can reduce (I-13, REST matches OpenSearch's recordings; ADR 0004, bounded memory).

## Change

`oss-reader` parses both as `AbstractPercentilesAggregationBuilder.createParser`'s `ConstructingObjectParser` does at OpenSearch 3.10.0: `field`, `percents` (or `values`) as one value or an array of numbers or strings read as `Double.parseDouble` reads them, `keyed`, and `tdigest.compression`; the keys are checked and sorted as `PercentilesAggregationBuilder.validatePercentiles` and the ranks constructor do, with their "Failed to build" errors. It serves them on integer, long and date fields at the top level. lucene-aggs builds the t-digest 3.3 `MergingDigest` value by value, as `TDigestState` does; `aggs::metric::tdigest` takes it over and ports the rest of `MergingDigest`: `mergeNewValues`, `merge`, `quantile`, `cdf`, and `AbstractTDigest.add(TDigest)` for `AbstractInternalTDigestPercentiles.reduce`. The response is `doXContentBody`'s, keyed or not, with `_as_string` for dates, `null` with no value, repeated keys kept, and NaN written as a string.

Not in this change: the `hdr` method, `missing`, `script`, `value_type`, `format`, a compression that is NaN, infinite or above 10,000 (OpenSearch fails or exhausts its heap far above it, and lucene-aggs keeps a digest's sort keys per thread), and the coordinator's reduce (R7). Each is refused with "not supported by this node yet".

## Acceptance criteria

- AC-1: the recorded percentiles searches (`fixtures/compat/search/paging-sort.json`: integer and date fields, default and given percents, ranks of numbers and dates, keyed and unkeyed, repeated keys, one percent given as a string, compression 5, an empty `tdigest`, no match, an unmapped field, NaN percents) get the status and body OpenSearch 3.10.0 gave, replayed against a reader node.
- AC-2: the recorded percentiles errors (`fixtures/compat/search/errors.json`: no percents, a percent out of range, ranks without or with no values, a negative or non-numeric compression, an unknown `tdigest` key, a `tdigest` or `percents` of the wrong type, a non-numeric percent, a non-boolean `keyed`, an unknown key, no field, a keyword field, a NaN rank value) get OpenSearch's status and body.
- AC-3: for 48 cases (values `0..n`, `i % 7` and pseudo-random, n from 1 to 10,000, compression 100 and 20), ten quantiles, ten cdfs, the size, and the same of three digests added into an empty one, equal bit for bit what OpenSearch 3.10.0's `TDigestState` printed.
- AC-4: on an index of four segments holding 0 to 199, `percentiles` and `percentile_ranks` give the values `TDigestState` gives for those values added in order: 2.0, 10.0, 49.5, 99.5, 149.5, 190.0 and 198.0 at the default percents, 1.0 at percent 1 with compression 5, and ranks 0.0, 25.5, 50.24999999999999, 99.75 and 100.0.
- AC-5: a search reserves each digest, the arrays `MergingDigest`'s constructor sizes for its compression, the engine's per-thread sort keys and the order a merge sorts, and its keys twice, with its hits; building, settling and reading a digest of up to five buffers of values allocates at most that.

## Test plan

`crates/compat/tests/node_search.rs` replays AC-1 and AC-2. `aggs/metric/tdigest_tests.rs` holds AC-3 (vectors printed by OpenSearch's own class), `aggs/metric/percentiles_tests.rs` the parsing, writing, reduce and digest sizes, and `aggs/metric/collect_tests.rs` AC-4 and AC-5, and `tests/digest_peak.rs` AC-5's peak under the counting allocator.

## S3 request budget

Unchanged: the digest reads the same doc values as the other metrics, in the same pass. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with `percentiles` was refused before.

## Risks

A digest's answers depend on every merge rule and on the order values arrive; a slip shows in AC-3's vectors. A concurrent-sliced OpenSearch shard adds slice digests together, which can change the last bits of a percentile, as for sums.
