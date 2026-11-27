# The overlay finds the commit's copies of the ids it replaces

## Serves

Parity plan R7.6, the tail overlay engine, second piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md)). Builds on [reader-overlay-segment](../reader-overlay-segment/SPEC.md). The searcher over commit plus overlay, the pool charge, the setting, the open-shard wiring and the request-count test against a blob-backed commit are later pieces; nothing calls this yet.

## Change

`oss-reader` gains, in `overlay`:

- `candidate_ids(records, local_checkpoint)`: the distinct ids with a record above the checkpoint, in first-seen order, refused past `MAX_IDS` as the build refuses.
- `find_copies(reader, ids)`: for each id with a live document in the commit's `DirectoryReader`, that document's segment, its number in the segment and its `_seq_no`, by one filter search of the `_id` terms sorted on `_seq_no`, then each hit's stored `_id`. It reads only through the reader it is given, so it adds no request beyond the lazy reads of that reader; it runs in the background, never in a search.
- `Overlay::build_over(mapping, records, checkpoint, max_bytes, copies)`: as `build`, but a record whose commit copy has a sequence number at or above its own is dropped (the commit already holds that version or a newer one), so the overlay never answers older than the commit.
- `Overlay::hidden(copies)`: the commit documents the overlay hides, one `(segment, document)` for each surviving id (replaced or deleted) that has a copy, and none for ids new to the commit.

## Acceptance criteria

- AC-1: `find_copies` returns, for each requested id present in the commit, the segment, the document number in it and the `_seq_no`, across several segments; an id the commit lacks is absent.
- AC-2: `candidate_ids` lists each id above the checkpoint once, in first-seen order, and refuses more than `MAX_IDS`.
- AC-3: `build_over` drops a record whose commit copy has an equal or higher sequence number and keeps one with a lower, so a replaced id with an older overlay entry is not in `ids()` and has no document.
- AC-4: `hidden` lists the commit copies of every surviving replaced or deleted id and none for new ids or dropped ones.
- AC-5: `find_copies` of no ids does nothing and reads nothing.
- AC-6: `find_copies` takes exactly `MAX_IDS` ids and refuses one more.

## Test plan

`crates/reader/tests/it/overlay_commit.rs`; commits are built in memory with `oss_doc`'s `Indexer`, so each document has `_id` and `_seq_no` as the writer writes them; one test per criterion.

## S3 request budget

`find_copies` makes only the reads the given reader makes lazily: blocks of the `_id` dictionary, its postings and the `_seq_no` doc values, at most the blocks of those structures, and none on a second call over the same reader (its page cache). A later piece asserts that count against a blob-backed commit, with the build kept off the search path.

## Bounds

At most `MAX_IDS` ids a call, so at most that many hits.

## Switch

None here: nothing calls it until the setting and wiring piece.

## Risks

The soft-deleted copies a commit keeps are hidden by the reader's soft-deletes wrapper before this runs, so only live copies are found; that is asserted when the shard is wired.
