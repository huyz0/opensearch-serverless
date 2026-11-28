# k-NN plugin API and engines, native

## Serves

The owner's rule (2026-11-28): no Java in the runtime path ([java-dependencies](../../java-dependencies.md), k-NN row). The Lucene engine's mapping, index, `knn` query and derived source are native ([knn-query](../knn-query/SPEC.md)); this spec closes what the audit lists as not served: `/_plugins/_knn/*`, the query options the node refuses, and the Faiss and NMSLIB engines.

## Change

Four slices, one PR each; the first is built with this spec.

1. **Stats, warmup, clear cache.** Routes of the stock plugin, served by the search service of each node and answering for that node only (`_nodes.total` 1, no fan-out):
   - `GET /_plugins/_knn/stats`, `/stats/{stat}`, `/{node}/stats`, `/{node}/stats/{stat}`. `{node}` is this node's id or `_local`, `_all`; another id is 404. Stats are process-wide counters the reader keeps: `knn_query_requests` and `knn_query_with_filter_requests` (top-k and radial `knn` nodes run, with and without a `filter`), `lucene_initialized` (a `knn` query has run), `faiss_initialized` and `nmslib_initialized` (false until slice 4), `circuit_breaker_triggered` (false), and the native-graph cache stats (`graph_memory_usage`, `graph_memory_usage_percentage`, `cache_capacity_reached`, `eviction_count`, `hit_count`, `miss_count`, `load_success_count`, `load_exception_count`, `total_load_time`, `graph_index_requests`, `graph_query_requests`, `graph_query_errors`, `indexing_from_model_degraded`, `indices_in_cache`, the `script_*` and `training_*` counters): all zero, `{}` or false, because the Lucene engine keeps no native graph cache. An unknown stat is 400 `illegal_argument_exception`. Reading stats makes no object-store request. With security on, stats need the plugin's `cluster:admin/knn_stats_action`, warmup `cluster:admin/knn_warmup_action` and clear cache `indices:admin/knn_clear_cache_action` on the named index.
   - `GET /_plugins/_knn/warmup/{index}[,{index}]`: opens every shard of each index as a search does and reads the vector files of its segments in one prefetch, so the first `knn` search finds them resident. Answers `{"_shards":{"total","successful","failed"}}`; an unknown index is 404 `index_not_found_exception`. Requests scale with the bytes of the vector files, once, not with time.
   - `GET /_plugins/_knn/clear_cache/{index}[,{index}]`: closes this node's open shards of each index, returning their `open_indexes` charge and engine state; the next search opens them again. A search that holds a shard finishes on it. Same answer shape and 404. It makes no object-store request. The shared page cache keeps its pages until its own policy ages them: the engine has no evict-by-file call, so a stricter clear needs a lucene-rust change and is not claimed here.
2. **Query options on the Lucene engine.** `ignore_unmapped` is served: with it `true`, a field the mapping lacks matches no document in all three modes (top-k, `min_score`, `max_distance`) and the rest of the query answers; a mapped field that is not a `knn_vector` still fails as before, and a non-boolean is the parse failure `failed to parse field [ignore_unmapped]`. Default `false`. `method_parameters` (`ef_search`), `rescore` and `expand_nested_docs` stay refused as unsupported: how the stock plugin serves them on the Lucene engine (which are accepted there, what `rescore` does with the Lucene engine, how nested hits expand) is not in this repository, and a recording from a stock 3.10 node is needed before they are written, as `knn-search` was.
3. **Models.** `GET|DELETE /_plugins/_knn/models/{id}` and `GET|POST /_plugins/_knn/models/_search`, over the document set `knn/models/` (a model is one document kept as the plugin's `_source`: `model_id`, `model_blob`, `state`, `timestamp`, `description`, `error`, `space_type`, `dimension`, `engine`; [ADR 0086](../../adr/0086-knn-engines-and-models-are-native.md)). A missing model is 404 `resource_not_found_exception` `Model "<id>" does not exist`; a delete of a model whose state is `training` is 400. Search serves `match_all` and `ids` (any other query would read every model and is 400), `from` and `size` (at most 100 a page), in id order, with `_index` `.opensearch-knn-models`. A model document is at most 10 MiB, which bounds the blob near 7 MiB; ADR 0086's 64 MiB needs a streaming read of a document and is not claimed here. Routes live behind `serverless.knn.enabled`; with security on they need `cluster:admin/knn_get_model_action`, `knn_delete_model_action` and `knn_search_model_action`. Nothing creates a model yet: `_train` is slice 4. Shapes (including the delete answer `{"model_id","result":"deleted"}`) follow the plugin's documentation and are provisional.
4. **Faiss and NMSLIB engines and training.** `POST /_plugins/_knn/models/{id}/_train` and `models/_train`: k-means (IVF) and product quantization trained natively over a `training_index`; HNSW under the `faiss` and `nmslib` engine names, served by the Lucene codec ([ADR 0086](../../adr/0086-knn-engines-and-models-are-native.md), stage 1); IVF and PQ search stay refused until that further ADR. Training's memory bound and the model store are fixed by that ADR; the IVF and PQ segment format (graphs held under the `open_indexes` pool, mapped not copied) needs a further ADR after an independent review before it is written. A native plugin-tier module ([ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md)) is the intended home if the engines are not linked into the reader.

Not here: Java engines, JNI, a Java fallback, cross-node fan-out of stats, and any cache that persists across process restarts.

## Acceptance criteria

Slice 1:

- AC-1: `GET /_plugins/_knn/stats` answers the stat set above with the plugin's names and types, under `nodes.<id>`, with `_nodes` `{total:1, successful:1, failed:0}`, `cluster_name`, `circuit_breaker_triggered` and `model_index_status` at the top.
- AC-2: running a top-k `knn` query raises `knn_query_requests` by 1 per `knn` node, a radial one too, and `knn_query_with_filter_requests` by 1 more when it has a `filter`; a search without a `knn` query changes neither.
- AC-3: `/stats/{stat}` answers only that stat; an unknown stat is 400; `/{node}/stats` with a node that is not this one is 404.
- AC-4: `warmup` on an index with vector fields makes the vector files resident (the prefetch is asked for every segment's vector ranges) and answers `_shards` total equal to the index's shard count; on an unknown index it is 404; it opens no shard twice.
- AC-5: `clear_cache` closes the index's open shards, returns exactly the bytes they were charged, and the next search opens afresh; on an index that is not open it still answers `_shards` with all shards successful and changes nothing; on an unknown index it is 404.
- AC-6: stats and `clear_cache` make 0 object-store requests; `warmup` makes only the requests of an open and a prefetch, asserted by the request meter.
- AC-7: with `serverless.knn.enabled` off, every route answers as an unknown route.
- AC-8 (slice 2): `ignore_unmapped: true` on an unmapped field matches nothing for `k`, `min_score` and `max_distance`, leaves a `bool`'s other clauses to answer, and does not hide a mapped non-vector field or a mapped vector field; `false` or absent fails as today.
- AC-9 (slice 2): `ignore_unmapped` parses as a boolean (or the strings `true` and `false`), defaults to false, and refuses another value.
- AC-10 (slice 3): a stored model reads back as its source; a missing one is 404 with the plugin's message.
- AC-11 (slice 3): a delete removes a model, a second delete is 404, and a model in state `training` is refused 400 and kept.
- AC-12 (slice 3): a search pages models in id order with `from` and `size`, serves `match_all` and `ids`, and refuses another query 400.
- AC-13 (slice 3): a model read costs the marker GET and the content GET and no LIST.
- AC-14 (slice 4, stage 1): a `faiss` method `hnsw` on a float field (spaces `l2`, `cosinesimil`, `innerproduct`; parameters `m`, `ef_construction`, `ef_search`; encoder `flat`) is accepted and written back with the engine the client named, in the plugin's key order; the dimension limit is 16000.
- AC-15 (slice 4, stage 1): each refusal the stock 3.10.0 makes for `faiss` and `nmslib` mappings is made with its message: `l1` and `linf` spaces, `sq` without `bits`, `pq` and `ivf` needing training, an unknown parameter, a non-positive or non-integer parameter, an unknown encoder, and `nmslib` for a new index.
- AC-16 (slice 4, `_train` request): `POST /_plugins/_knn/models/{id}/_train` and `/models/_train` refuse a body the stock 3.10.0 plugin refuses with its exception type and message (missing `training_index` or `dimension`, no `method`, the `lucene` engine, an `hnsw` method with nothing to train, unknown or non-positive parameters), a model id already stored, a training index or field that does not exist, and a dimension that differs from the field's. A request that passes every check is answered 501 until the training job is built; nothing is stored.

## Test plan

AC-1 to AC-3: counters unit-tested in `crates/reader/src/knn`, the routes by HTTP in `crates/node/tests/server`. Each test is written first and seen red against a stub that answers `{}`. AC-4 to AC-6: a node test over the emulator with the request meter and the pool's reserved bytes before and after.

## S3 request budget

Stats and clear cache: 0 requests. Warmup: the open's requests plus the ranged reads of the vector files, once per call; nothing scales with records, indexes or elapsed time. Idle indices make none.

## Switch

`serverless.knn.enabled` (existing). No `native_engines` switch (supersedes [ADR 0086](../../adr/0086-knn-engines-and-models-are-native.md) on this point, per the owner's rule that plugins need no enable flag): a `faiss` mapping is served by the codec the `lucene` engine already uses, so it has no cost of its own, and a stored mapping must load whatever a switch said. No new setting: the routes are new paths an index without a vector field never reaches, and a counter bump is one relaxed atomic add.

## Risks

Response shapes come from the plugin's published documentation, not a recording of a stock node; they are provisional until recorded. Stats are per node, where the plugin merges every node's, so a client summing `nodes` sees one entry. `clear_cache` does not evict shared page-cache pages.
