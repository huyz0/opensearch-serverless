# Reader: sub-aggregations of a numeric `terms`

## Serves

Roadmap R10.2: the last parent of the sub-aggregation series ([fixed-bucket-subs](../fixed-bucket-subs/SPEC.md), [keyed-bucket-subs](../keyed-bucket-subs/SPEC.md)): I-13 (REST matches OpenSearch), ADR 0004 (bounded memory).

## Change

`oss-reader` accepts `aggs` under a `terms` of a numeric field: a single-value or stats metric, or a keyword `terms`. Each sub-aggregation's engine node goes under the parent's `NumericTerms` node; the engine's buckets give each kept bucket its list of values (`aggs::bucket::keyed_subs`). Shard results reduce by term key (`sub_values::reduce_keyed`), keeping each value with its bucket after the order and size cut, and each bucket writes its sub-aggregations after `doc_count`. A bucket aggregation, or a numeric `terms`, under it is refused as "not supported by this node yet".

## Acceptance criteria

- AC-1: each bucket holds the metric sub-aggregations of the documents it counts.
- AC-2: a keyword `terms` under a bucket counts that bucket's documents.
- AC-3: a `terms` over an unmapped field has no bucket and the empty values.
- AC-4: a numeric `terms` under a numeric `terms` fails with a 400 naming it.
- AC-5: a reduce adds each key's sub-aggregations and keeps them with the kept buckets.
- AC-6: the response writes each bucket's sub-aggregations after its `doc_count`.
- AC-7: a search reserves for the field's value count (at most the bucket limit); a keyword `terms` reserves nothing here.
- AC-8: no request is added (the row of [request-budgets.md](../../request-budgets.md) for the term search with aggregations).

## Test plan

Unit tests in `aggs/bucket/terms_subs_tests.rs`; the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-8).

## S3 request budget

Unchanged (AC-8): LIST 0, HEAD 0, PUT 0.

## Switch

None: the request was refused before, and one without sub-aggregations is unchanged.

## Risks

No stock 3.10.0 recording backs these steps.
