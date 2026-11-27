# A searcher over the commit and the overlay

## Serves

Parity plan R7.6, the tail overlay engine, third piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md)). Builds on [reader-overlay-segment](../reader-overlay-segment/SPEC.md) and [reader-overlay-commit](../reader-overlay-commit/SPEC.md). The pool charge, the setting, the open-shard wiring and the fetch on open are later pieces; `search` does not call this yet.

## Change

`oss-reader` gains, in `overlay`:

- `Masks::new(commit, hidden)`: for each commit segment, its live documents with the hidden documents cleared (the segment's own live documents, or all documents when it has none). A segment with nothing hidden keeps no mask.
- `combine(commit_opened, masks, overlay_opened, commit_max_doc)`: the commit's open segments, each with its mask where it has one, followed by the overlay's segment placed after the last commit document. This is the segment list `IndexSearcher::new` takes, so aggregations, top hits and fetch see the overlay through it with no special case.
- `Overlay::norms(fields)`: the overlay segment's norms for the scored fields, to follow the commit's in the searcher's norms list.

The commit's files and reader are never changed; a mask is a copy.

## Acceptance criteria

- AC-1: a searcher over `combine` finds the overlay's documents and not the commit's copies of the ids it replaces or deletes: a term query on a replaced document's old value finds nothing, on its new value finds one; a deleted id finds nothing; a new id is found; an untouched commit document is found.
- AC-2: with no overlay and no masks the segment list is the commit's own and answers as the commit does.
- AC-3: a mask keeps the commit's own deletions: a document the commit had already removed stays hidden.
- AC-4: a scored text query over `combine` with `Overlay::norms` returns the commit's and the overlay's matching documents with positive scores.
- AC-6: a hidden document the commit does not hold (an unknown segment, a negative or out-of-range number) is ignored, and an overlay with no document has no norms.
- AC-5: the overlay's documents are numbered after the commit's: the global document number of an overlay document is the commit's `max_doc` plus its number in the overlay.

## Test plan

`crates/reader/tests/it/overlay_view.rs`, one test per criterion, over commits built with `oss-doc`; the shared helper is `crates/reader/tests/common/mod.rs`.

## S3 request budget

Zero added: masks are copies in the heap and the overlay is in memory; the searcher reads the commit as it does now.

## Bounds

A mask is a bit set the size of a commit segment's `max_doc`, charged with the overlay by the wiring piece (ADR 0018 states a sparse bound; this piece keeps a bit set per segment that has hidden documents and the wiring charges `max_doc / 8` for each). At most 10,000 hidden documents.

## Switch

None here: nothing calls it until the setting and wiring piece.

## Risks

Scoring statistics are the searcher's sum over its segments (the commit counts its masked documents in document frequency, as Lucene counts deleted documents); parity with the shell's `OverlayReadOnlyEngine` is measured in the usable-cluster suite, not here.
