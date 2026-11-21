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

Those of step 1; each later step adds its own here with its evidence.

- AC-1: a `range`, `filters`, `filter`, `histogram`, `date_histogram` or numeric `terms` under a `range`, `filters`, `filter`, `histogram`, `date_histogram` or numeric `terms` bucket counts that bucket's documents.
- AC-2: a nested reduce counts every bucket it merges or fills in one running total; over the bucket limit it fails with `too_many_buckets_exception` (the status the shard's own refusal uses), and a fill past the limit is that failure, not a mismatch.
- AC-3: a search reserves for each nested level's buckets, `B_k`, at the per-bucket bytes times `SUB_COPIES`: a data-keyed level holds at most its field's values times the buckets one document can land in above it (the width of each `range` or `filters` above, the bucket limit above a multi-valued field, else one), a `range` its width times its parents, never past the bucket limit.
- AC-4: a keyword `terms` with a bucket aggregation or a numeric `terms` under it is still refused with a 400 naming it.
- AC-5: a nested numeric `terms` whose (parent, value) pairs pass the bucket limit fails with `too_many_buckets_exception` instead of running.
- AC-6: a cold search with a bucket aggregation nested to four levels adds no request to the same search without the nesting.

The compiled queries of nested `filters` and `filter` are planned beside the top-level ones in one depth-first list a top-level aggregation (`filters_collect::Queries`).

## Test plan

Unit tests in `aggs/bucket/nested_tests.rs` and `slice_tests.rs`; the engine's key cap has its own test in lucene-rust (#567); the budget gate in `crates/node/tests/it/aggs_budgets.rs` (step 4).

## S3 request budget

Unchanged (step 4): LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before, a request without nesting is unchanged, and over the limit it fails rather than runs (ADR 0022).

## Risks

No stock 3.10.0 recording backs these steps; expected bodies are read from the Java.
