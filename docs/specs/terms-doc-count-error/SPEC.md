# Reader and coordinator: `show_term_doc_count_error` of a terms aggregation

## Serves

Roadmap R10.2, after [terms-agg](../terms-agg/SPEC.md) and [keyword-terms-agg](../keyword-terms-agg/SPEC.md), which refused `show_term_doc_count_error` (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`show_term_doc_count_error` is read as `TermsAggregationBuilder`'s `declareBoolean` reads it (a boolean, or the string "true" or "false"). When true, every bucket of a numeric or keyword `terms` writes `"doc_count_error_upper_bound"` after its `doc_count` and before its sub-aggregations (`InternalTerms.Bucket.toXContent`).

A bucket's bound is what `InternalTerms.reduce` and `reduceBucket` leave: each shard takes its own error (`getDocCountError`, the one already summed into the aggregation's `doc_count_error_upper_bound`) off the bound stored in each bucket it holds, and the sum of every shard's error is added back, so a bucket holds the sum of the errors of the shards that did not return it; -1 when any shard's error is unbounded. A result reduced again keeps its stored bounds. Shard results store none (0 each); results carry a `bucket_errors` list beside `buckets`, empty unless the request asked.

Not in this change: `shard_min_doc_count`, which is [terms-shard-min-doc-count](../terms-shard-min-doc-count/SPEC.md).

## Acceptance criteria

- AC-1: with `show_term_doc_count_error`, two full shards `[(1,10),(2,4)]` and `[(2,6),(3,5)]` reduce to buckets 1 and 2 with bounds 5 and 0, and the aggregation's bound is 9; the written bucket carries `"doc_count_error_upper_bound":5` after `doc_count`.
- AC-2: a single shard's buckets all have bound 0; without the option nothing is written.
- AC-3: when a shard's error is unbounded (-1, an ascending count order), every bucket's bound is -1.
- AC-4: a keyword `terms` reduces and writes the same bounds.
- AC-5: the coordinator's reduce of shards carries the bounds.
- AC-6: the option parses from a boolean.
- AC-7: a key held by several shards takes its bound as `reduceBucket` adds it: the parts' bounds in order, each less its shard's error, and the addition stops once the running sum is -1 (the value an unbounded error also has). Shards of errors (1, 2), (2, 1), (1, 1) and (3, 2) holding one key leave it 2, 0, 1 and 0, the bytes of `fixtures/terms-error/java-terms-error-3.10.0.txt`.

## Test plan

`aggs/bucket/terms_tests.rs`, `aggs/bucket/keyword_terms_tests.rs` and `crates/coordinator/tests/it/terms_reduce.rs`.

## S3 request budget

Unchanged: nothing is read beyond the shard's buckets. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with the option was refused before.

## Risks

The bound follows the Java's reduce read at 3.10.0 and recorded from its own classes for the key-held-by-several-shards cases (AC-7); the sum stops at -1, so a bound depends on the order the parts are reduced in, as Java's does on the order shards answer. A reduce of results that did not store bounds (a mixed fleet with another build) is not possible: results are in-process values.
