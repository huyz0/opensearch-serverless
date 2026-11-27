# Reader: `significant_terms` aggregation

## Serves

Roadmap R10.2: OpenSearch's `significant_terms` bucket aggregation over a keyword field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `significant_terms` (`SignificantTermsAggregationBuilder` at OpenSearch 3.10.0): `field`, `size` (10), `shard_size`, `min_doc_count` (3), `shard_min_doc_count` and the `jlh` (default) and `percentage` heuristics, each an empty object. It runs the engine's `SignificantTerms` node (`terms: true`, keys as terms) for each term's foreground count and the shard's matching documents (`subsetSize`), then reads each candidate's background count from the terms dictionaries (`LeafReader.docFreq` summed over the segments, `SignificanceLookup`) and the background size (`IndexReader.maxDoc()`), scores it with the heuristic, and keeps the best `size` with at least `min_doc_count` documents (the shard's `LocalBucketCountThresholds`). The reduce sums the shards' sizes and counts, rescores with the global sizes, keeps the buckets with `doc_count >= min_doc_count` and a score above 0, best score first, ties by term, and the answer is `{"doc_count", "bg_count", "buckets": [{"key", "doc_count", "score", "bg_count"}]}`.

Not in this change, each refused as "not supported by this node yet": `background_filter`, `include`, `exclude`, `execution_hint`, `missing`, the `mutual_information`, `chi_square`, `gnd` and `script_heuristic` heuristics, a numeric field, sub-aggregations, and a `significant_terms` under another aggregation.

## Acceptance criteria

- AC-1: the body parses with its defaults (size 10, min_doc_count 3); `size` and `shard_size` of 0 or less, a negative count, an unknown key and an unserved key fail as `terms` does.
- AC-2: `jlh` and `percentage` score as `JLHScore` and `PercentageScore` do, including a background count of 0 (read as 1 by `jlh`, score 0 by `percentage`).
- AC-3: a shard's buckets are the best `size` by score among terms with `min_doc_count` documents; the reduce sums sizes and counts across shards, rescores and drops scores of 0 or less.
- AC-4: over an index, the foreground is the query's matches, the background every document of the shard, and the answer carries both sizes and each bucket's counts.
- AC-5: a keyword field the shard does not map answers no bucket; a numeric field, a sub-aggregation and a nested `significant_terms` are refused.

## Test plan

Unit tests in `aggs/bucket/significant_tests.rs` (AC-1 to AC-3, AC-5) and over an `oss-testkit` index (AC-4). Expected scores come from reading the Java heuristics; no stock recording exists yet.

## S3 request budget

The foreground pass reads the field's doc values and dictionaries as `terms` does; the background counts read the same dictionaries (a seek a candidate term and segment, through the page cache). LIST 0, HEAD 0, PUT 0; an idle index makes none. Requests do not scale with documents or indexes.

## Switch

None: a `significant_terms` was refused before, and a request without one is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. The shard keeps `size` buckets (OpenSearch's non-concurrent path), so `shard_size` and `shard_min_doc_count` are validated and not used. Ties in score are broken by term here; OpenSearch's priority queue leaves them to hash order.
