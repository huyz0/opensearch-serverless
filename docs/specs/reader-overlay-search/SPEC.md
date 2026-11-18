# Search over a commit and its tail overlay

## Serves

Parity plan R7.6, the tail overlay engine, fourth piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md)). Builds on [reader-overlay-view](../reader-overlay-view/SPEC.md). The setting, the background builder and the fetch on open are later pieces; nothing calls this from a node yet.

## Change

- `OpenShard::with_overlay(&overlay, &copies)`: a second `OpenShard` over the same commit whose reader is the commit's segments (with the documents the overlay replaces or deletes hidden) followed by the overlay's segment. `search`, aggregations, sorting and `write_response` take it as they take any shard, so no search path changes.
- `OpenShard::directory` becomes an `Arc`, so a view shares the commit's directory instead of reopening it.
- `DirectoryReader::with_live_docs_masks` in lucene-rust applies the masks; the pin moves to a commit that holds it.
- `Overlay::view_of` and `Masks::into_live`: the combined reader a view holds.

The commit's reader, files and the original `OpenShard` are never changed.

## Acceptance criteria

- AC-1: a term query over the view finds a replaced id's new value and not its old one, and `_source` of the hit is the overlay's; a deleted id is not found; a new id is found.
- AC-2: a sort on a long field over the view ranks an overlay document among the commit's by its value, and a max aggregation sees the overlay's values.
- AC-3: a view over an overlay with no document answers every query as the commit does.
- AC-4: after a view is made the original shard still answers as the commit alone.

## Test plan

`crates/reader/tests/overlay_search.rs`, one test per criterion, over an index of 40 documents in four segments built with `oss-testkit` and records built in the test.

## S3 request budget

Zero added: a view reads the commit through the same directory and page cache, and the overlay is in memory. AC-1 asserts the meter is unchanged by a search over the view once the commit's pages are cached.

## Bounds

A view holds clones of the commit's segment readers (reference counts) and one bit set of `max_doc / 8` bytes for each commit segment with a hidden document. The overlay's charge does not yet include those bytes: the wiring piece adds their sum to the pool charge and tests the limit.

## Switch

None here: nothing calls it until the setting and wiring piece.

## Risks

A mask is a copy of a segment's live documents; a stale view over a newer commit is the caller's to drop (the wiring piece rebuilds the view when the commit changes).
