# The `knn` query over a lucene-engine `knn_vector` field

## Serves

Parity plan R15, k-NN native, step 3 of four. Steps 1 to 2c store a vector the way the stock plugin does; this step searches them: `{"query":{"knn":{"<field>":{"vector":[..],"k":N}}}}`, as the stock plugin answers it.

## Change

- Recorded from stock OpenSearch 3.10.0 with its k-NN plugin, one index a space (`l2`, `cosinesimil`, `innerproduct`): `fixtures/compat/knn/knn-search-*.json`. Scores, `k`, `size`, `from`, `filter`, `boost`, `bool` composition and every refusal below are what that node answered.
- Parsing (`crates/reader/src/dsl/queries/knn.rs`): one field, with `vector` (a non-empty array of numbers), `k` (1 to 10,000), `filter` (a query) and `boost`. The stock messages are kept: the missing-`k` message, the `k` range message, the missing and empty vector messages, `unknown field`, the number failure. `min_score`, `max_distance`, `method_parameters`, `rescore`, `expand_nested_docs` and `ignore_unmapped` are refused as unsupported by this node, not ignored.
- Execution: a `knn` query is not a per-segment clause; the engine's `knn_top_hits` is Lucene's `AbstractKnnVectorQuery.rewrite`: the best `k` of the whole shard, `filter` applied while the graph is walked, and the result a `DocAndScoreQuery`. The search opens the shard's segments, computes each `knn` node's result in the order the tree is read, and the query builder takes those results as it builds the tree. The rest of the query (`bool`, `boost`, paging, `size`, `track_total_hits`) is unchanged, so `k` is a per-shard count, as in the plugin.
- A `knn` query outside the search request's `query` (an aggregation filter, `post_filter`, `count`, `validate`) is refused as unsupported.
- The reader's mapping reads `knn_vector` with its `dimension`; every other use of the field type stays unsupported (sort, aggregations, term-level queries).
- No switch: it is a new query name that an index without a vector field can never match, and the index it needs is already behind `serverless.knn.enabled`.

## Acceptance criteria

- AC-1: for each space, the recorded `knn-search-*` search steps over documents the Rust writer indexed return the recorded ids, order, scores and totals, for `k`, `k` larger than the vectors, `size`, `from`, `filter`, a filter that matches nothing, `boost`, `bool` `must`/`should`, `track_total_hits: false` and a document with no vector or a `null` one.
- AC-2: every recorded refusal gives the recorded status, `type` and `reason`.
- AC-3: `min_score`, `max_distance` and the other parameters not served are refused with the unsupported message and no search runs.
- AC-4: the `knn` query in `post_filter`, an aggregation filter, `count` and `validate` is refused as unsupported.
- AC-5: a field that is not a `knn_vector` (unmapped, keyword) and a vector of the wrong dimension fail as the plugin's `query_shard_exception`.
- AC-6: an index with no knn query reads the same blobs and makes the same requests as before (the reader's request budgets are unchanged).

## S3 request budget

The vector files are read with the segment's other files: `.vec`, `.vex` and `.vem` go through the same pooled range reads, so a `knn` search adds GETs only for the files of a field it searches and no round trip beyond the rank phase's. Measured by the existing budget tests staying green and one new one over a knn index.

## Switch

None. See Change.

## Risks

Recall: the search runs Lucene's HNSW as the plugin does with `k` as the beam; the fixtures are small enough that every space is exact, so a recall difference is not caught here. Radial search is not served until its distance-to-score rule per space is recorded.
