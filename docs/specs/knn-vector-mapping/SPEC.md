# The `knn_vector` mapping, as the k-NN plugin keeps it

## Serves

Parity plan R15, k-NN native. The Java shell runs the stock k-NN plugin, so a mapping the plugin wrote must read here, and one written here must read there. This is the first of three steps: the mapping (this spec), a document's vector and the `_source` the plugin derives from it, then the `knn` query. Until the later two exist nothing indexes or searches the field.

## Change

- `oss-doc`'s mapping gains the `knn_vector` type for the shape this writer will index: `dimension`, an optional `data_type` of `float`, `meta`, and a `method` of `hnsw` on the `lucene` engine with an optional `space_type` (`l2`, `cosinesimil`, `innerproduct`; the method's, or the field's top-level one, or `l2`) and `parameters` `m` and `ef_construction` (positive integers). Ported from `KNNVectorFieldMapper.TypeParser`, `KNNMethodContext.parse` and the Lucene HNSW method's validation (k-NN 3.10.0.0): the plugin's refusals, with its exception types and messages, and its write-back (`engine`, `space_type`, `name`, `parameters`, with `ef_construction` before `m`, the order of Java's `HashMap`).
- A valid mapping outside that shape is unsupported, never invalid, so a router keeps it on a Java writer: a field with no method (the plugin indexes it as doc values), a method with no engine (the plugin resolves Faiss), the `faiss` and `nmslib` engines, `data_type` `byte`, `binary` and `half_float`, `store`, `doc_values`, `mode`, `compression_level`, `model_id` and a top-level `engine`.
- `Mapping::parse_with(text, ParseOptions { knn })`: `knn: None` (what `Mapping::parse` passes) keeps the type unsupported, so nothing changes for any caller until the document and query steps wire it; `Some(flag)` indexes it on an index whose `index.knn` is `flag`, and a method on an index without `index.knn` is refused as the plugin does.
- `fixtures/compat/mapping/knn-*.json` are recorded from the stock OpenSearch 3.10.0 release (tarball, SHA-512 checked against the published sum) with its bundled k-NN plugin: creates, `GET _mapping`, `PUT _mapping` merges and 60 refusals. `fixtures/compat/knn/` holds the document fixtures the next step replays.

## Acceptance criteria

- AC-1: every step of `fixtures/compat/mapping/knn-*.json` replays: an accepted mapping is written back as the plugin's `GET _mapping` shows it and reads back to the same mapping, a merge conflicts or succeeds as the plugin's does, and a refusal carries the plugin's exception types and messages down its `caused_by` chain.
- AC-2: a caller that does not opt in (`Mapping::parse`) is told `knn_vector` is unsupported.
- AC-3: each shape listed above as outside the shape is unsupported, not invalid.
- AC-4: a method on an index whose `index.knn` is false is refused with the plugin's message.
- AC-5: the order of `parameters` in a mapping is not part of the mapping, and a method reads back to the mapping that wrote it.
- AC-6: when a method names a space type different from the top-level one, the plugin's conflict is reported before the engine's own checks.

## S3 request budget

None: mapping parsing makes no request.

## Switch

`ParseOptions::knn` is the switch for now: off in every caller. The node setting that turns it on arrives with the document step, when there is something to turn on.
