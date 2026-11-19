# Reader: `terms` aggregation over a keyword field

## Serves

Roadmap R10.2, after the numeric [terms-agg](../terms-agg/SPEC.md): OpenSearch's `terms` on a keyword field of a single-shard `_search`, with a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` serves `terms` over a keyword field with the same keys as the numeric one: `field`, `size`, `shard_size`, `min_doc_count` (at least 1) and `order` (`_count` or `_key`, one criterion). The shard counts with lucene-aggs' keyword `terms` (the port of `GlobalOrdinalsStringTermsAggregator`) over the field's global ordinals, keeps what `StringTerms` holds (`aggs::bucket::keyword_terms::KeywordTermsResult`: keys as text), reduces shard results as `InternalTerms.reduce` (reached through `partial::reduce`, so `oss-coordinator`'s `reduce_shards` serves it) and writes it as `StringTerms.doXContentBody` does. The parser and `TermsSpec` are the numeric aggregation's; the shard picks the result by the field's mapped type.

Global ordinals are built per search from the segments' term dictionaries. Their memory is reserved with the hits before the search starts, from sizes the segments' metadata states (`terms_dict_size`, `max_term_length`), so nothing is sized by reading the dictionaries first: a field whose dictionaries do not fit the pool refuses the search, rather than growing without bound.

Not in this change, each refused with "not supported by this node yet": `min_doc_count: 0` on a keyword field (it makes every term of the shard a bucket), `include`, `exclude`, `missing`, `script`, `value_type`, `format`, `execution_hint`, `collect_mode`, `show_term_doc_count_error`, `shard_min_doc_count`, sub-aggregations, and `order` by a sub-aggregation or with several criteria. A text field still fails as OpenSearch fails it.

## Acceptance criteria

- AC-1: on an index of several segments, the buckets of a keyword field are its distinct terms with their document counts over the query's matches, ordered by count descending then term ascending, cut at `size`, with `sum_other_doc_count` the documents of the buckets cut; `order` by `_key` and by `_count` ascending list as OpenSearch lists them; keys compare as bytes.
- AC-2: `min_doc_count` above 1 is applied by the reduce and counts the dropped buckets' documents in `sum_other_doc_count`; an unmapped field and a match-none query give no buckets; `min_doc_count: 0` and every key listed above are refused as not supported yet.
- AC-3: shard results reduce as `InternalTerms.reduce` does for the numeric one (equal keys add, `min_doc_count` filters, the order sorts, `size` cuts into `sum_other_doc_count`, `doc_count_error_upper_bound` is 0 for one shard and otherwise the sum of what each full shard may have cut); results of different kinds (a keyword and a numeric `terms` of one name) do not reduce.
- AC-4: the response writes `doc_count_error_upper_bound`, `sum_other_doc_count` and `buckets`, each bucket `key` (a JSON string, escaped as OpenSearch escapes it) then `doc_count`.
- AC-5: the search reserves a keyword `terms`' memory with its hits: for each segment holding the field, 40 bytes a term of its dictionary (the ordinal maps and the counts), twice its longest term (the merge's term buffers), and for each of `shard_size` buckets (at most the bucket limit and the terms there are) three copies of the longest term of any segment and 64 bytes of headers and counts (the engine's key, the result's and a reduce's); a field whose mapping is not a keyword is reserved as before.
- AC-6: the cold and warm requests of a term search with a keyword `terms` beside its metrics are held to the rows `search.keyword_terms.*` of [request-budgets.md](../../request-budgets.md).

## Test plan

Unit tests in `aggs/bucket/keyword_terms_tests.rs` (reduce, writing: AC-3, AC-4), `aggs/bucket/keyword_collect_tests.rs` over an `oss-testkit` index of four segments (AC-1, AC-2, AC-5), `crates/coordinator/tests/keyword_terms_reduce.rs` (AC-3 across shards), the budget gate in `crates/node/tests/it/aggs_budgets.rs` (AC-6).

## S3 request budget

Measured, not assumed: AC-6 holds the cold request and round-trip counts of a keyword `terms` beside the metric aggregations. LIST 0, HEAD 0, PUT 0; an idle index makes none.

## Switch

None: a keyword `terms` was refused before, and a request without one is unchanged.

## Risks

No stock 3.10.0 recording backs these steps, so the expected bodies are read from the Java. The reservation is read from segment metadata: an upper bound on the terms, not a measurement of the engine's allocations. A multi-shard search reserves the hits of shard 0 for every shard (`crates/node/src/coordinate.rs`), so shards with larger dictionaries than shard 0 are under-reserved; that path belongs to the node assembly. A keyword `terms` of an index mapped as a keyword in one place and unmapped in another does not reduce with the unmapped shard's empty numeric result. Global ordinals are rebuilt on every search of an open reader that has not kept them; a reader that keeps them holds their bytes beyond the request.
