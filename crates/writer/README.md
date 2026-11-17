# oss-writer

The indexing role. Appends to the node's one write log, builds segments, uploads each as one immutable blob, publishes commits.

## Owns
The per-node write log and its fencing, index mappings (parse, merge, dynamic growth), segment build and upload, commit publish, and pushing tail records to readers.

## Depends on
`oss-types`, `oss-blob`, `oss-format`, `oss-rest` (its JSON tokenizer, for mappings), `chunkjson` (pull tokens, for documents). Never `oss-reader`.

## Entry points
`src/lib.rs`; `log::NodeLogStore`, one node's write log by name: appends (hedged or not), fences, header and slice reads, deletes; `log::NodeLogCommitter`, group commit of every shard's records into one PUT a window, with epochs in the lease and trimming. `mapping::Mapping`, an index's mapping: `parse` from a request or descriptor, `merge` an update, `to_json` in the form `GET _mapping` shows, refusing what this writer cannot index as unsupported ([spec](../../docs/specs/mappings/SPEC.md)); `document::parse`, a document read against a mapping: each field's values, `_ignored`, and the dynamic update and grown mapping ([ADR 0017](../../docs/adr/0017-mappings-and-documents-as-the-java-reads-them.md)). Built second; see [docs/roadmap.md](../../docs/roadmap.md).
