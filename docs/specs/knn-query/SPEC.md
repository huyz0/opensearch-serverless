# The `knn` query over a lucene-engine `knn_vector` field

## Serves

Parity plan R15, k-NN native, step 3 of four. Steps 1 to 2c store a vector the way the stock plugin does; this step searches them: `{"query":{"knn":{"<field>":{"vector":[..],"k":N}}}}`, as the stock plugin answers it.

## Change

- Recorded from stock OpenSearch 3.10.0 with its k-NN plugin, one index a space (`l2`, `cosinesimil`, `innerproduct`): `fixtures/compat/knn/knn-search-*.json`. Scores, `k`, `size`, `from`, `filter`, `boost`, `bool` composition and every refusal below are what that node answered.
- Parsing (`crates/reader/src/dsl/queries/knn.rs`): one field, with `vector` (a non-empty array of numbers), exactly one of `k` (1 to 10,000), `min_score` (above 0) and `max_distance`, `filter` (a query) and `boost`. The stock messages are kept: the missing-`k` message, the `k` range message, the missing and empty vector messages, `unknown field`, the number failure. `method_parameters`, `rescore`, `expand_nested_docs`, `ignore_unmapped` and `_name` are refused as unsupported by this node, not ignored.
- Execution: a `knn` query is not a per-segment clause; the engine's `knn_top_hits` is Lucene's `AbstractKnnVectorQuery.rewrite`: the best `k` of the whole shard, `filter` applied while the graph is walked, and the result a `DocAndScoreQuery`. The search opens the shard's segments, computes each `knn` node's result in the order the tree is read, and the query builder takes those results as it builds the tree. The rest of the query (`bool`, `boost`, paging, `size`, `track_total_hits`) is unchanged, so `k` is a per-shard count, as in the plugin.
- Radial forms (`knn-radial-*` fixtures): a per-segment query, not a rewrite: every document scoring at least a threshold, found by Lucene's radial search with the plugin's greediness, `filter` applied while the graph is walked, hits unbounded by `k`. `min_score` is the Lucene score itself; `max_distance` becomes a score per space as the stock answers show: l2 `1 / (1 + d)`, cosinesimil `(2 - d) / 2`, innerproduct the score of a product of `d` (`d + 1`, or `1 / (1 - d)` below zero). l2 and cosinesimil refuse a negative distance and a score above 1 (`failed to create query: ...`); innerproduct refuses neither. The mapping keeps each vector field's `space_type` (from `method` or the field, `l2` by default).
- A top-k `knn` query outside the search request's `query` (an aggregation filter, `post_filter`, `count`, `validate`) is refused as unsupported.
- The reader's mapping reads `knn_vector` with its `dimension`; every other use of the field type stays unsupported (sort, aggregations, term-level queries).
- No switch: it is a new query name that an index without a vector field can never match, and the index it needs is already behind `serverless.knn.enabled`.

## Acceptance criteria

- AC-1: for each space, the recorded `knn-search-*` search steps over documents the Rust writer indexed return the recorded ids, order, scores and totals, for `k`, `k` larger than the vectors, `size`, `from`, `filter`, a filter that matches nothing, `boost`, `bool` `must`/`should`, `track_total_hits: false` and a document with no vector or a `null` one.
- AC-2: every recorded refusal gives the recorded status, `type` and `reason`.
- AC-3: the parameters not served are refused with the unsupported message and no search runs; the recorded `knn-radial-*` steps (`min_score`, `max_distance`, `max_distance` with a filter) return the recorded ids, order and scores in all three spaces; each space's threshold is the one the stock node's answers imply, and the refusals are the plugin's.
- AC-4: a `knn` query in `post_filter`, and in any tree built without the search's answers (an aggregation filter, `validate`), is refused as unsupported; the mapping reads a vector field's `dimension` and no other field's.
- AC-5: a field that is not a `knn_vector` (unmapped, keyword) and a vector of the wrong dimension fail as the plugin's `query_shard_exception`.
- AC-6: a search without a `knn` query opens its segments where it did before and runs no vector search; the reader's request budgets are unchanged.

## S3 request budget

A search without a `knn` query is unchanged (the existing budget tests stay green). A `knn` search reads the vector files of the field it searches through the same pooled range reads as the segment's other files; their request count on the emulator is measured in [docs/knn-measurements.md](../../knn-measurements.md) (a `knn` search adds none to the open's), and the same over RustFS 1.0.1.

## Switch

None. See Change.

## Risks

Recall: the search runs Lucene's HNSW as the plugin does with `k` as the beam; the fixtures are small enough that every space is exact, so a recall difference is not caught here. The radial rule per space was inferred from the stock node's answers at thirteen distances and fifteen scores a space, not read from the plugin's source; a boundary the probes did not cover could differ by rounding.
