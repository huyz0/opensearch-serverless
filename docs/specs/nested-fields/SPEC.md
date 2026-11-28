# Nested fields: mapping and child documents

## Serves

Parity task R10.1/R10.2, second slice of nested support (the reader's `nested` query is [reader-nested-query](../reader-nested-query/SPEC.md)); invariant I-13 (REST matches OpenSearch); principles, switches and defaults. Changes what an index stores, so it is high risk and needs an owner's go before merge.

## Change

An index may map an object as `type: nested`. A document's nested objects are indexed as hidden child Lucene documents written as one block with the document, children first and the document last, as `DocumentParser.nestedContext` and `IndexWriter.softUpdateDocuments` do at OpenSearch 3.10.0.

- **Mapping** (`oss-doc`): `type: nested` with `properties` and `dynamic`, as a plain object; `include_in_parent` and `include_in_root` of `true` are refused as unsupported (`false` is accepted). `toXContent` writes `"type":"nested"` first. Merging a nested object with a plain one, either way, fails `cannot change object mapping from nested to non-nested` or `... from non-nested to nested`. At most 100 nested objects (`index.mapping.nested_fields.limit`) and 10,000 nested documents in one document (`index.mapping.nested_objects.limit`), with the Java's messages. A `knn_vector` under a nested object is refused as unsupported.
- **Parse** (`oss-doc`): each object of a nested path, one per array element, is a child document holding the fields read inside it under their full names (`c.w`); the parent's fields omit them. A child that holds nested objects is written after its own children, so a block is ordered by completion, the document last. New fields under an existing nested object are added to it by a dynamic update without a conflict.
- **Lucene document** (`oss-doc`): a child holds its fields, `_id` (indexed, not stored, so the soft-delete by `_id` term reaches the block) and `_nested_path` (the path, indexed): no `_seq_no`, `_primary_term`, `_version` or `_source`. The document keeps all of those, so `_primary_term` is what tells the reader a root from a child.
- **Writer** (`oss-writer`): an index, update and replay hands the engine the block in one call; a delete writes its one tombstone, which the `_id` term deletes with the whole block. The fast ingest plan declines an index with a nested object. The version lookup already takes the last live document of an `_id`, the root.
- **Reader overlay** (`oss-reader`): the unpublished documents of the log are built as blocks, and a replaced or deleted document hides its children in the commit as it hides itself.
- **Switch** (`oss-node`): `serverless.nested.enabled`, on by default (`OSS_NESTED=false` turns it off); a create that maps a `nested` object is refused as unsupported while it is off, as it was before the flip. A stored mapping is read whatever the setting is now, as `knn_vector` is. Put-mapping does not add a nested object yet.

Not here: `inner_hits`, nested aggregations and `reverse_nested`, `include_in_parent` and `include_in_root` of `true`, vectors in nested objects, a nested object added by put-mapping, `copy_to` into or out of a nested object.

## Acceptance criteria

- AC-1: with nested on, a mapping with nested objects (also inside nested objects and inside plain ones) parses, round-trips through `to_json` as OpenSearch writes it, and a refusal is the Java's or the node's one unsupported message; with it off, every nested object is refused as today.
- AC-2: merging nested with non-nested fails with the Java's two messages, the same nested object merges, and a nested object past 100 fails.
- AC-3: a document's nested objects parse to child documents in completion order with the fields inside them; arrays give one child an element; nested inside nested gives the inner children first; an empty array, a null and a missing object give none; a concrete value where a nested object is mapped fails as an object does.
- AC-4: a document with more than 10,000 nested objects fails with the Java's message.
- AC-5: a dynamic field under an existing nested object, and a dynamic object holding one, are added to the nested object's mapping and indexed in its child.
- AC-6: the Lucene block has a document a child plus one, the children carrying only their fields, `_id` and `_nested_path`, the last the root with its metadata; the same root as before for a document without nested objects.
- AC-7: indexing a document with children, updating it with fewer, and deleting it leaves the live documents the shard should hold: the version lookup finds the root, the update's old children are gone, the delete removes them all.
- AC-8: an index with a nested object makes no fast-ingest plan.
- AC-9: through a node, with the switch on: create, bulk index, a `nested` query finds the parents, the total counts parents, an update replaces the children, a delete removes them, a get returns the source; with a document still only in the log (not yet published) the same answers hold.
- AC-11: an index of four shards searched through one node: the nested query's total counts parents of every shard, a page sorted by a parent field and one sorted by a nested field merge in order, `inner_hits` return every matching child, and `nested`, `reverse_nested` and a `terms` under them add up across shards.
- AC-10: with the switch off a create with a nested mapping is refused 400 with the unsupported message; an index created with it on and the node then restarted off still reads and writes.

## Test plan

- AC-1, AC-2: `crates/doc/tests/nested_mapping.rs`.
- AC-3 to AC-5: `crates/doc/tests/nested_document.rs`.
- AC-6: `crates/doc/tests/nested_lucene.rs`.
- AC-7, AC-8: `crates/writer/tests/it/nested_shard.rs`.
- AC-9, AC-10: `crates/node/tests/nested.rs`; AC-11: `crates/node/tests/nested_shards.rs`.

## S3 request budget

Unchanged per operation: a block is one write call and one blob a segment, as the document would be. A document with children holds more Lucene documents in the same segment, so a segment's bytes grow with the children, not the request count. LIST 0, HEAD 0, PUT 0 added. Idle: 0.

## Memory

Indexing memory is reserved from each record's bytes, which already contain the children (they are part of `_source`); the children's postings add to the segment buffer in proportion. The reader's parent bit set per segment and join node is reserved with the search's hits ([nested-memory](../nested-memory/SPEC.md)). The indexing side is not reserved separately here: NOT-RUN until a measured bound exists.

## Switch

`serverless.nested.enabled`, node scope, on by default since the release-build soaks in VERIFIED.md held the bound; it was off until then. It flips on when a 10,000-document nested soak holds the write p99 within 5% of a plain index's and the search tests pass in both modes (not measured here: emulator numbers are provisional). Both values are tested (AC-10); with it off the code path is the old one.

## Risks

- Block ordering: a merge or a segment split that separates a block breaks the join. The engine keeps blocks together; AC-7 and AC-9 hold the shard to it.
- A child with the same `_id` term as its root widens every `_id` lookup to the block: the version lookup, the overlay's hiding and a delete all take the last or all of them as the Java does.
- Nested queries and aggregations are replayed against fixtures recorded from stock 3.10.0 (`fixtures/compat/nested-search/`); only `min` under a nested has no stock fixture, and expected values for it are read from the Java.
