# Reader: `shard_min_doc_count` of a terms aggregation

## Serves

Roadmap R10.2, after [terms-agg](../terms-agg/SPEC.md), [keyword-terms-agg](../keyword-terms-agg/SPEC.md) and [terms-doc-count-error](../terms-doc-count-error/SPEC.md), which refused `shard_min_doc_count` (I-13, REST matches OpenSearch).

## Change

`shard_min_doc_count` is a non-negative long (a negative one fails as `TermsAggregationBuilder.shardMinDocCount` fails: "[shardMinDocCount] must be greater than or equal to 0. Found [n] in [name]"). `BucketCountThresholds.ensureValidity` caps it at `min_doc_count`, so the shard threshold is `min(shard_min_doc_count, min_doc_count)`, 0 when unset. A shard drops the buckets of fewer documents than that, and their documents count in `sum_other_doc_count` (the shard adds every bucket's documents to the others and takes the kept ones back off). It serves numeric and keyword `terms`, at the top level and under another bucket aggregation, with their sub-aggregation values dropped alongside.

The engine picks a shard's `shard_size` buckets before the threshold applies; OpenSearch applies the threshold first. Under the count-descending order the two agree (the dropped buckets are the smallest, and a dropped one cannot make room for another). Under any other order they differ, so a request whose capped threshold is above 0 and whose order is not `_count` descending is refused with "not supported by this node yet".

## Acceptance criteria

- AC-1: the key parses (a number or a string), is capped at `min_doc_count`, defaults to 0, and a negative value fails as OpenSearch fails it.
- AC-2: a threshold above 0 with an order other than count descending is refused as not supported yet; one capped to 0 serves any order.
- AC-3: a shard's buckets under the threshold are dropped with their sub-aggregation lists and their documents join `other_doc_count`: on a real numeric and a real keyword shard, and on a result with sub-aggregation lists.

## Test plan

`aggs/bucket/terms_tests.rs`, `keyword_terms_tests.rs`, `collect_tests.rs` and `keyword_collect_tests.rs`.

## S3 request budget

Unchanged: the filter runs on the engine's kept buckets. LIST 0, HEAD 0, PUT 0.

## Switch

None: a request with the key was refused before.

## Risks

Not yet recorded against a stock node. The other orders stay refused until the engine takes the threshold itself.
