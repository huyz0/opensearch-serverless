# The reader builds an overlay segment from pushed writes

## Serves

Parity plan R7.6, the tail overlay's reader engine, first piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), [ADR 0019](../../adr/0019-document-indexing-crate-below-reader-and-writer.md)). Builds on [reader-tail-held](../reader-tail-held/SPEC.md), whose records it takes. Finding the commit's copies of the overlay's ids and the live-documents mask, the search wiring, the pool charge, the setting and the node-side source are later pieces; nothing calls this yet.

## Change

`oss-reader` gains `overlay::Overlay`, built by `Overlay::build(mapping, records, local_checkpoint)` from `oss_format::WalRecord`s in ordinal order and the index's `oss_doc` mapping.

- Records at or below `local_checkpoint` are dropped. Of the rest, the one with the highest sequence number for an id is kept (the later record when equal), so an id has one version.
- A kept index record becomes a document in one in-memory Lucene segment, written with `oss_doc`'s `Indexer` into a `ByteBuffersDirectory`, with the metadata the record carries (id, sequence number, primary term, version, source). A kept deletion becomes no document.
- `ids()` lists every id the overlay replaces or deletes (kept index records and kept deletions), in first-seen order: what a later piece masks in the commit.
- At most `MAX_IDS` (10,000) distinct ids; more is `BuildError::TooManyIds`, which the caller turns into a skipped shard. A record the mapping or the engine cannot take is `BuildError::Record { id, .. }`, the same.
- `reader()` is the segment's `DirectoryReader`, open for searching; `doc_count()` its documents.
- `estimate_bytes(records)` is an upper bound on the heap a build of those records takes, used for the pool charge: the measured test asserts it is at least the built overlay's measured size.

## Acceptance criteria

- AC-1: records at or below the checkpoint are dropped, and an id with several records keeps the highest sequence number.
- AC-2: an index record is a searchable document with its id, source and sequence number; a deletion is no document and its id is in `ids()`.
- AC-3: `ids()` holds each replaced or deleted id once, in first-seen order.
- AC-4: more than `MAX_IDS` distinct ids is `TooManyIds` and builds nothing; exactly `MAX_IDS` builds.
- AC-5: a source the mapping refuses is `Record` naming the id.
- AC-6: no records above the checkpoint is an empty overlay (no documents, no ids).
- AC-7: `estimate_bytes` is at least the built overlay's measured heap on a realistic batch, and grows with the record bytes.

## Test plan

`crates/reader/tests/overlay_segment.rs`; one test per criterion. The documents are read back through the segment reader, not the builder's own state.

## S3 request budget

Zero: the segment is in memory and the builder never touches a blob store.

## Bounds

At most `MAX_IDS` documents. The records come from `TailHeld`, bounded at 100,000 records a shard; the build's memory is charged by a later piece before it runs, from `estimate_bytes`.

## Switch

None here: nothing calls the builder until the setting and wiring piece.

## Risks

The segment's field numbers and norms are its own; the searcher over commit plus overlay must read norms per segment (a later piece). The in-memory directory is kept alive with the reader.
