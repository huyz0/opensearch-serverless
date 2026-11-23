# The writer keeps acknowledged writes until a commit holds them

## Serves

Parity plan R7.6, the tail overlay's writer half. The keeping side of the shell's `TailPush` (`acked`, `published`, `released`, `keptAfter`, `keeps`): a writer holds every write it acknowledged and has not published, so a reader that opens an older commit can be given what the commit lacks. The push to readers, the fetch handler and the reader's overlay are later pieces.

## Change

- `oss-node` gains `tail_keep::TailKeep`, memory only, per (index uuid, shard):
  - `acked(uuid, shard, records, push)` keeps the records above what was published. With `push` false it begins the shard's keep and stores nothing, as the shell does for a write that asked not to be pushed.
  - `published(uuid, shard, checkpoint)` drops every record at or below the checkpoint and remembers it, so a later ack at or below it is not kept.
  - `kept_after(uuid, shard, after)` gives the kept records above `after`, oldest first.
  - `complete_after(uuid, shard, after)` says whether a reader holding a commit at `after` gets every acknowledged write from `kept_after`: false below the floor (the shard held writes before the keep began), below the last published checkpoint (those records are discarded) and below the highest sequence number dropped for want of room (a commit at that number holds it).
  - `released(uuid, shard)` frees the shard.
- Not changed: pushing to readers, the fetch action, grants, the reader's overlay, the setting that turns overlays on, and the call from the write path. Nothing calls `TailKeep` yet, so no switch is added here.

## Acceptance criteria

- AC-1: records acked are returned by `kept_after`, oldest first, only those above `after`.
- AC-2: `published` drops records at or below the checkpoint, frees their bytes, and a later ack at or below it is not kept.
- AC-3: the keep begins with a floor of the lowest acked sequence number minus one; `complete_after` is false below it and true at it.
- AC-8: `complete_after` is false below the last published checkpoint and true at it.
- AC-4: past 20,000 records or 16 MiB for a shard the oldest are dropped, and `complete_after` is false below the highest dropped sequence number and true at it.
- AC-5: past 256 MiB across the node a record is not kept and counts as dropped; bytes fall again as shards publish or are released.
- AC-6: a write with `push` false stores nothing but begins the keep with its floor.
- AC-7: `released` frees the shard's bytes, and the shard is not kept afterwards; a shard never acked answers no records and `complete_after` false.

## Test plan

`crates/node/tests/tail/tail_keep.rs`, with limits passed in so the bound tests stay small; each criterion has one test. The bounds are also asserted at the real constants.

## S3 request budget

Zero: memory only.

## Bounds

20,000 records and 16 MiB per shard, 256 MiB per node (the shell's `MAX_KEPT_PER_SHARD`, `MAX_KEPT_BYTES_PER_SHARD`, `MAX_KEPT_BYTES_PER_NODE`); a record counts as the bytes of its id and source. Freed on publish and release.

## Switch

None yet; the setting arrives with the wiring.

## Risks

A byte count that drifts would leak the node bound shut; the tests assert it returns to zero after publish and release.
