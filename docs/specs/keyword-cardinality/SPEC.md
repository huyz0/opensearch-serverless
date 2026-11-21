# Reader: cardinality of a keyword field

## Serves

Roadmap R10.3, the reader half, continued from [cardinality-agg](../cardinality-agg/SPEC.md), which left a keyword field for the global ordinals' memory to be reserved: `cardinality` of a keyword field on a reader's single-shard `_search` (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`cardinality` on a keyword field counts the distinct terms of the matches as `CardinalityAggregator.OrdinalsCollector` does: lucene-aggs sets a bit for each global ordinal seen, then hashes each distinct term with `MurmurHash3` and collects it into the `HyperLogLogPlusPlus` sketch of the requested precision. The sketch is the one a numeric `cardinality` writes, so the reduce, the estimate and the response are unchanged. `execution_hint` is validated as before and both modes give the same sketch.

A search reserves, with its hits, the field's ordinals (`ORD_BYTES` a term of each segment's dictionary), the term buffers read while hashing, and one bit a term for the owning bucket, beside the sketch every `cardinality` reserves.

Not in this change: `value_count` of a keyword field, `cardinality` as a sub-aggregation of a bucket aggregation, `missing`, `script` and `value_type`; each is refused as before.

## Acceptance criteria

- AC-1: over four segments each holding all sixteen terms of a keyword field, `cardinality` is 16, with and without `execution_hint`; over the matches of a query it counts those matches' terms; over no match the sketch is empty.
- AC-2: a search reserves the keyword field's ordinals beyond what a numeric `cardinality` reserves.
- AC-3: `value_count` of a keyword field is still refused with "[value_count] of the keyword field [tag] is not supported by this node yet".

## Test plan

`aggs/metric/collect_tests.rs` holds all three.

## S3 request budget

Unchanged: the ordinals are built from the same dictionary reads as a keyword `terms`. LIST 0, HEAD 0, PUT 0.

## Switch

None: a keyword `cardinality` was refused before.

## Risks

The hash of a term must be the one OpenSearch uses or a mixed fleet estimates differently; lucene-aggs hashes the term's bytes with `murmur3_h1` at seed 0, as `MurmurHash3.hash128`'s first half. The expected estimates are not yet recorded against a stock node.
