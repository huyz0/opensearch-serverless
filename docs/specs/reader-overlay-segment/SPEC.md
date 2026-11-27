# The reader builds an overlay segment from pushed writes

## Serves

Parity plan R7.6, the tail overlay's reader engine, first piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), [ADR 0019](../../adr/0019-document-indexing-crate-below-reader-and-writer.md)). Builds on [reader-tail-held](../reader-tail-held/SPEC.md), whose records it takes. Finding the commit's copies of the overlay's ids and the live-documents mask, the search wiring, the pool charge, the setting and the node-side source are later pieces; nothing calls this yet.

## Change

`oss-reader` gains `overlay::Overlay`, built by `Overlay::build(mapping, records, local_checkpoint, max_bytes)` from `oss_format::WalRecord`s in ordinal order and the index's `oss_doc` mapping.

- Records at or below `local_checkpoint` are dropped. Of the rest, the one with the highest sequence number for an id is kept (the later record when equal), so an id has one version.
- A kept index record becomes a document in one in-memory Lucene segment, written with `oss_doc`'s `Indexer` into a `ByteBuffersDirectory`, with the metadata the record carries (id, sequence number, primary term, version, source). A kept deletion becomes no document.
- `ids()` lists every id the overlay replaces or deletes (kept index records and kept deletions), in first-seen order: what a later piece masks in the commit.
- At most `MAX_IDS` (10,000) distinct ids; more is `BuildError::TooManyIds`, which the caller turns into a skipped shard. A record the mapping or the engine cannot take is `BuildError::Record { id, .. }`, the same. The sequence number, primary term and version are written by `oss_doc` from the record and covered by that crate's fixtures, not re-read here.
- `max_bytes` is what the caller reserved from the pool before calling; the build computes `estimate_bytes` over the records it keeps (not the dropped or superseded ones) and refuses with `BuildError::TooManyBytes` before allocating anything for the index if that is over `max_bytes`.
- `reader()` is the segment's `DirectoryReader`, open for searching (`None` for an overlay with no document); `doc_count()` its documents.
- `estimate_bytes(records)` is an upper bound on the heap a build of those records takes at its peak, used for the pool charge before the build.

## Acceptance criteria

- AC-1: records at or below the checkpoint are dropped, and an id with several records keeps the highest sequence number.
- AC-2: an index record is a searchable document with its id, and its stored source is the winning record's; a deletion is no document and its id is in `ids()`.
- AC-3: `ids()` holds each replaced or deleted id once, in first-seen order.
- AC-4: more than `MAX_IDS` distinct ids is `TooManyIds` and builds nothing; exactly `MAX_IDS` builds.
- AC-5: a source the mapping refuses is `Record` naming the id.
- AC-6: no records above the checkpoint is an empty overlay (no documents, no ids).
- AC-7: `estimate_bytes` is at least the peak heap of a build, measured by the counting allocator over batches from one record to 512 KiB sources, and at most eight times it or 512 KiB; it grows with the records.
- AC-8: a build with `max_bytes` exactly the estimate of the kept records runs, one byte less is `TooManyBytes`, and records below the checkpoint or superseded add nothing to the estimate.

## Test plan

`crates/reader/tests/it/overlay_segment.rs`; one test per criterion; AC-7 in its own binary, `crates/reader/tests/overlay_heap.rs`, because the counting allocator is process-wide. The documents are read back through the segment reader, not the builder's own state.

## S3 request budget

Zero: the segment is in memory and the builder never touches a blob store.

## Bounds

At most `MAX_IDS` documents and `max_bytes` by the estimate of the kept records, both refused before the index is written. The records come from `TailHeld`, bounded at 100,000 records a shard; the caller reserves `estimate_bytes` of what it holds before calling.

## Switch

None here: nothing calls the builder until the setting and wiring piece.

## Risks

The segment's field numbers and norms are its own; the searcher over commit plus overlay must read norms per segment (a later piece). The in-memory directory is kept alive with the reader.
