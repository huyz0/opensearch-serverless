# oss-doc

A source and a mapping as the Lucene document OpenSearch indexes for it. Shared by the writer and the reader's tail overlay ([ADR 0019](../../docs/adr/0019-document-indexing-crate-below-reader-and-writer.md)).

## Owns
Index mappings (parse, merge, `GET _mapping` form), a document read against a mapping, and the Lucene fields built from it: metadata, postings, norms, doc values, points and stored values.

## Depends on
`oss-rest` (its JSON tokenizer, for mappings), `chunkjson` (pull tokens, for documents) and lucene-rust's `lucene-index`, `lucene-codecs` and `lucene-analysis`; its tests also use `lucene-search` and `lucene-store`. Never `oss-reader` or `oss-writer`.

## Entry points
`src/lib.rs`; `mapping::Mapping`, an index's mapping: `parse` from a request or descriptor, `merge` an update, `to_json`, refusing what this crate cannot index as unsupported ([spec](../../docs/specs/mappings/SPEC.md)); `document::parse`, a document read against a mapping ([ADR 0017](../../docs/adr/0017-mappings-and-documents-as-the-java-reads-them.md)); `lucene::Indexer`, the Lucene documents as OpenSearch writes them, each field registered through `lucene::FieldNumbers` ([spec](../../docs/specs/indexing/SPEC.md)).
