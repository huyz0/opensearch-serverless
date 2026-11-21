# Reader: a bucket aggregation under a bucket

## Serves

Roadmap R10.2, after the sub-aggregation series ([numeric-terms-subs](../numeric-terms-subs/SPEC.md)): I-13 (REST matches OpenSearch), ADR 0004 (bounded memory), [ADR 0022](../../adr/0022-nested-bucket-aggregations.md).

## Change

Four steps, one PR each. The accepted design is ADR 0022; this file holds the criteria.

1. A `range`, `histogram` or `date_histogram` as a sub-aggregation of a `range`, `filters` or `filter` (`filters`, `filter` and a numeric `terms` as sub-aggregations follow, once their compiled queries and key cap reach a nested node): the slice of an engine result per owning bucket (`aggs::bucket::slice`), the nested value in each bucket, a reduce that counts buckets as it merges.
2. The same under a `histogram` or `date_histogram`.
3. The same under a numeric `terms`.
4. The reservation across levels (`B_k`), and the limit tests against the budget rows.

A keyword `terms` as the parent of a bucket aggregation stays refused until a later change.

## Acceptance criteria

- AC-1: a bucket aggregation under a `range`, `filters` or `filter` bucket counts that bucket's documents (step 1).
- AC-2: a result sliced per owning bucket equals the result of running the sub-aggregation alone over that bucket's documents (step 1).
- AC-3: a nested reduce counts every bucket it merges or fills in one running total; over the bucket limit it fails with `too_many_buckets_exception` (the status the shard's own refusal uses), and a nested reducer's failure is an error, never the first shard's value (step 1).
- AC-4: the same as AC-1 under a `histogram` or `date_histogram` (step 2) and under a numeric `terms` (step 3).
- AC-5: a nested numeric `terms` whose (parent, value) pairs pass the bucket limit fails with `too_many_buckets_exception`, without holding more than the limit's keys (step 3).
- AC-6: a search reserves for each nested level's buckets, `B_k`, at the per-bucket bytes times `SUB_COPIES` (step 1 for a nested `range`, `histogram` or `date_histogram`; step 4 for the rest).
- AC-7: no request is added (the budget row for the term search with aggregations; step 4).
- AC-8: a keyword `terms` with a bucket aggregation under it is still refused, naming it (every step).

## Test plan

Unit tests in `aggs/bucket/nested_tests.rs` and `slice_tests.rs`; the engine's key cap has its own test in lucene-rust (#567); the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-7).

## S3 request budget

Unchanged (AC-7): LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before, a request without nesting is unchanged, and over the limit it fails rather than runs (ADR 0022).

## Risks

No stock 3.10.0 recording backs these steps; expected bodies are read from the Java.
