# 0065: A nested object is a child document in its parent's block

**Status:** Accepted after independent review, 2026-11-26. Changes the index format, so the PR carries `needs-huy` for the owner's go before it merges.

## Context

The reader serves the `nested` query over blocks the Java shell writes, but no index made by this node can hold a nested mapping, so a nested index stays on a Java writer. Lucene joins a nested object to its parent by position: the children are adjacent documents written just before the root, and a parent bit set marks the roots.

## Decision

1. A `type: nested` object is a mapping that holds a child document for each of its objects, as OpenSearch 3.10.0 writes them. A write is one block in one `add` call: the children in completion order, then the root. A child carries `_id` (indexed, not stored, so deleting or replacing by the `_id` term reaches the whole block), its fields, and `_nested_path` (the object's full path, indexed as one term). Only the root carries `_seq_no`, `_primary_term`, `_version`, `_source`, `_routing` and the doc values for them.
2. A document may hold at most 10,000 nested objects (`index.mapping.nested_objects.limit`), and a mapping at most 100 nested fields, with the Java's messages. `include_in_parent` and `include_in_root` of true, and a `knn_vector` under a nested object, are refused as unsupported.
3. The complete path writes blocks. The fast ingest plan declines a document whose mapping has a nested object, so it never writes half a block. The reader's overlay builds an unpublished document as the writer does.
4. **Switch:** `serverless.nested.enabled`, node scope, off by default (`OSS_NESTED`). It governs only a create that maps a nested object; an index already holding one is read and written whatever the switch says, because the stored mapping is parsed as a descriptor. It flips on by default when a 10,000-document nested soak holds the write p99 within 5% of a plain index's and the search tests pass in both modes.

## Consequences

- No request changes: a block is one write call, and a segment is still one blob. A segment's bytes and Lucene document count grow with the children; a search's total and `_count` still count roots, because a search filters children out. Counts read from the Lucene document count (`_stats`, `_cat/indices` docs.count) are not checked and may include children.
- A merge must keep a block together. The engine's merge keeps documents in order and nothing in the writer splits a segment; `a_merge_keeps_every_block_together` holds it through soft-updates and a merge.
- A mapping update through `PUT _mapping` cannot add a nested object yet; dynamic mapping never creates one.
- Memory: a search of a nested index builds a parent bit set a segment (one bit a Lucene document) through the engine's `QueryBitSetProducer`, lazily and cached; it is not metered by the memory budget, and a per-segment meter is a follow-up. The overlay hides a replaced document's whole block with an iterator over the block's documents, so no list of them is held; its masks are one bit set a segment with a hidden document, as before. Reading a block's start scans one segment's `_primary_term` doc values up to its last copy, in the background build, never inside a search.

## Alternatives rejected

- Flatten nested objects into the parent (the `object` behaviour): wrong answers for a query that needs two fields to match in one object.
- Store children in a separate index: a second write path and a join that costs requests.

## Review

Independent review 2026-11-26 by a reviewer given the ADR, the principles and the diff: no conflict with the S3 cost, bounded memory, scalability, one write log, one blob a segment or switch principles. It asked for the memory bound on the hidden documents (done: an iterator, no list), a merge test (added), narrower claims for counts and the parent bit set (done), and the gap that no node test holds a nested document only in the log beside a published one (recorded in VERIFIED.md).
