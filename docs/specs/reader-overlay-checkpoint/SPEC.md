# The commit's checkpoint and the index's mapping source

## Serves

Parity plan R7.6, the tail overlay engine, sixth piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md)). The background builder needs two facts about the commit it builds over: the local checkpoint the commit holds, and the index's mapping as the document builder reads it.

## Change

- `overlay::local_checkpoint(reader)`: the `local_checkpoint` the writer stored in the commit's user data (`oss_writer::index::LOCAL_CHECKPOINT_KEY`); `-1` (no operation performed, as the Java's `NO_OPS_PERFORMED`) when the commit has none or it is not a number. Reads nothing: the user data is in the `segments_N` the reader already holds. The soft-deletes wrapper keeps it.
- `IndexMeta::mapping_json`: the mapping's JSON source, unescaped, the text `oss_doc::mapping::Mapping::parse` takes. Empty when the descriptor holds no mapping (not tested: no fixture has none). The open-cost estimate counts its bytes.

Why the user data and not a search for the highest `_seq_no`: a deleted document is a soft-deleted tombstone the wrapper hides, so the highest live sequence number can lie below the commit's checkpoint, and an index record the commit already superseded would come back.

## Acceptance criteria

- AC-1: a commit whose user data holds `local_checkpoint` gives that number, through the soft-deletes wrapper as well.
- AC-2: a commit with no such key, or one that is not a number, gives `-1`.
- AC-3: `IndexMeta::mapping_json` of an opened index is its mapping's JSON source, and parses to the mapping `Mapping::parse` reads from the same text.
- AC-4: the open-cost estimate is not below the heap an open retains with the mapping text held (the existing `open_cost_heap` bound).

## Test plan

`crates/reader/tests/overlay_checkpoint.rs` for AC-1 and AC-2 over in-memory commits; `crates/reader/tests/it/open.rs` for AC-3; AC-4 is `crates/reader/tests/open_cost_heap.rs`.

## S3 request budget

Zero added.

## Switch

None: a pure read of data already in memory.

## Risks

A commit written by the Java shell holds `local_checkpoint` in its user data as OpenSearch writes it; a writer that never stored it gives `-1`, so the builder treats every held record above `-1` as a candidate and the commit's copies decide.
