# 0039: A `knn_vector` is written into the segment as the stock k-NN plugin writes it

**Status:** Accepted after independent review, 2026-11-24

## Context

A Java node runs the stock k-NN plugin over the same blobs, so a segment the Rust writer publishes with a vector field must be one the plugin reads. Recorded from stock OpenSearch 3.10.0 (`fixtures/segments/knn-*.json`): a `lucene`-engine HNSW field is plain `Lucene99HnswVectorsFormat` files, a `FieldInfo` with the dimension, `FLOAT32` and a similarity by space, and two per-field format attributes; the plugin's own vector formats are not involved. Nothing here adds a request: a segment is one blob whatever it holds.

## Decision

- The indexer returns a document's vectors beside the document, and the writer hands both to lucene-rust, which writes the flat vectors and the HNSW graph (`m`, `ef_construction` from the mapping; defaults 16 and 100). `Indexer::document` keeps its signature, so a write path with no vector field builds what it built before.
- The fast ingest plan declines a mapping with a vector field; the complete path takes it.
- A node setting `serverless.knn.enabled` (`OSS_KNN`), off by default, makes the index admin accept `knn_vector`. Index settings cannot be given at create yet, so it stands for `index.knn`. A stored mapping is read with the type allowed, because it was checked on the way in. Off, an index with the type is refused as unsupported and stays on a Java writer. It turns on once the `knn` query serves the field.
- The tail overlay does not build vector documents: an index with unpublished vector writes is served from the commit alone until the overlay takes them.

## Derived source (amendment)

- Stock stores a knn index's `_source` with each vector's value replaced by `1` and puts it back on read, under codec `KNN1040Codec` with `derived_vector_fields` in the segment's `.si` (recorded in `fixtures/segments/files/`). A shard whose mapping has `knn_vector` fields opens its engine that way (`oss_writer::index::knn::open_writer`), and stores `_source` masked by a byte splice of each vector's value (`oss_doc::lucene::derived`). A source the splice cannot place (an escaped or dotted key over a vector, a vector inside an array of objects) is refused as unsupported and stays on a Java writer.
- Reading goes through lucene-rust's `visit_source_document`, which splices vectors back into a segment that lists derived fields and reads every other segment as stored. The search fetch and the realtime get use it, so a knn index a Java writer wrote is read correctly here.
- No setting: it is the plugin's behaviour for every knn index, and the type is already behind `serverless.knn.enabled`. No request is added; `_source` of a vector index is smaller by the vectors.
- A Rust-written segment's vectors count once in the indexing buffer and are masked in `_source`, so the memory bound above holds with a source shorter by the vectors' text.

## Consequences

- Java and Rust read each other's vector segments; checked by the fixtures and by Lucene 10.6.0 `CheckIndex` on Rust-written segments.
- Segment size grows by the vectors and the graph.
- Memory is bounded as for any document: a vector is charged to the engine's indexing buffer when the document is buffered (`dimension × 4` bytes, checked by `vectors_are_charged_to_the_indexing_buffer`), the node's indexing-buffer limit flushes by it, and a mapping's `dimension` is at most 16,000 (Lucene's own limit, `MAX_DIMENSION`). A vector field takes one vector a document. The graph is built at flush from the buffered vectors, so its peak is the buffer's bytes plus about `m` links of four bytes per vector per level; that peak is not measured yet, which is a reason the setting is off by default: it is measured, and the buffer limit's headroom checked against it, before the setting is turned on.
- Until the overlay takes vector documents, an index with unpublished vector writes is slower to become searchable.
