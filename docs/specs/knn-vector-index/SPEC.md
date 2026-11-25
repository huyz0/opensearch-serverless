# Indexing a `knn_vector` into the segment, as the k-NN plugin writes it

## Serves

Parity plan R15, k-NN native, step 2b of four. The mapping (`knn-vector-mapping`) and a document's value (`knn-vector-document`) are in; this spec writes the vector into the shard's Lucene segment so a Java node, which runs the stock k-NN plugin over the same blobs, reads it, and so step 3 can search it. Step 2c keeps `_source` without the vector.

## Change

- The segment layout is what stock OpenSearch 3.10.0 with its bundled k-NN plugin writes for a `lucene`-engine HNSW field (`fixtures/segments/knn.json`, recorded by `record.py`): plain `Lucene99HnswVectorsFormat` files, no plugin format; the field's `FieldInfo` has the vector dimension, encoding `FLOAT32` and a similarity function by space (`l2` Euclidean, `cosinesimil` cosine, `innerproduct` maximum inner product) and the attributes `PerFieldKnnVectorsFormat.format` and `.suffix`; a document with no vector has none.
- `oss-doc`'s `Indexer` registers the field's `FieldInfo` with its vector shape, and returns each document's vectors beside its `ExplicitDocument`; `Indexer::document` keeps its signature and a new method returns the pair, so every caller that never sees a vector is unchanged.
- The writer passes the vectors to the engine (`add_explicit_documents_with_vectors`, lucene-rust 569) and gives each vector field its graph's `m` and `beam_width` (`ef_construction`, default 100 and 16 as the plugin's `Lucene99HnswVectorsFormat` is built).
- The complete ingest path takes the field; the fast ingest plan declines a mapping with a vector field (it adds nothing to a plan today).
- A node setting turns the type on: `serverless.knn.enabled`, off by default for a node, (`OSS_KNN` at boot), which makes the index admin parse a created mapping with `ParseOptions { knn: Some(true) }`, so an index with `knn_vector` is still refused as unsupported (and stays on a Java writer) until it is set. Index settings cannot be given at create yet, so the node setting stands for `index.knn`. A mapping a descriptor stores was checked when it was accepted: it is read with `knn_vector` allowed, whatever the setting is now.
- Known limit: the reader's tail overlay cannot build a document with a vector, so it serves the commit alone for an index with unpublished vector writes until step 2c or 3 lifts it.

## Acceptance criteria

- AC-1: for each document of `fixtures/segments/knn.json`, the Lucene document the Indexer builds has the same fields, the same `FieldInfo` (dimension, encoding, similarity, attributes) and the same vector as the stock segment records, for `l2`, `cosinesimil` and `innerproduct`.
- AC-2: a shard that indexes vector documents, flushes and reads back through the reader returns each vector bit for bit and counts the documents with a vector.
- AC-3: `CheckIndex` of Lucene 10.6.0 (the Java one, from the stock distribution's `lucene-core` jar) passes on a segment the Rust writer wrote with vector documents, deletes and an update, and reads the vectors back equal.
- AC-4: with the node setting off, creating an index with a `knn_vector` field answers as before (unsupported, no change); with it on, the index is created and a document with a vector is indexed.
- AC-5: a write path that never meets a vector field makes the same requests and builds the same documents as before (the existing writer fixtures and request-budget tests pass unchanged).

## S3 request budget

None added: a segment with vectors is published as one blob like any other, and the vector files are part of it. A segment's size grows by its vectors and graph.

## Switch

`serverless.knn.enabled`, a node setting, on by default since the `knn` query serves the field and [docs/knn-measurements.md](../../knn-measurements.md) measured no downside (no request added, flush memory bounded by the buffer). It was off until then. `OSS_KNN=false` is the old behaviour, and the tests that pin it set it explicitly; the new default has its own.
