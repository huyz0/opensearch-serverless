# The reader holds the writes its writer pushed

## Serves

Parity plan R7.6, the tail overlay's reader half, first piece: the receiving store of the shell's `TailOverlays` (`take`, `handle`, `track`, `untrack`, `skipped`, `nextOrdinal`, `from`, `answered`, `granted`). Builds on [writer-tail-push](../writer-tail-push/SPEC.md) and [writer-tail-fetch](../writer-tail-fetch/SPEC.md). The engine that searches these records on top of a commit, the fetch a reader makes when it opens a shard, and the transport handler are later pieces.

## Change

`oss-node` gains `tail_held::TailHeld` (`reader` feature), memory only, per (index uuid, shard). Times are passed in as milliseconds, so it holds no clock.

- `take(uuid, shard, records, now)` stores the records above the shard's local checkpoint under consecutive ordinals and returns how many (`None`: unwanted). Unwanted is a shard no engine tracks and none tracked in the last 30 s, a shard that refused for want of room, and one already holding 100,000 records (the shell's `MAX_DOCUMENTS * 2`), which sets the shard refusing.
- `track` (an engine opened on a commit at a local checkpoint) raises the checkpoint, drops held records at or below it, and clears refusing and skipped; `untrack` ends one and, at none, starts the 30 s keep and lets go of every shard whose keep ran out; `skipped` marks that an engine left a record out.
- `next_ordinal` and `from(ordinal)` read what is held.
- `answered(uuid, shard, writer, asked, answer, sent_at)` takes a fetch's records and its grant as the shell's `answered` does: no subscription clears the writer's grant; a subscription anew records writer, id and whether the answer was complete, with a grant that started at the send; one that renews the held subscription extends the grant only.
- `granted(uuid, shard, writer, now)` is true while an engine is open, nothing was refused or skipped, the answer was complete (an answer with no subscription clears the grant, so no separate subscription test is needed), the writer is the one granting, and the grant has not run out.
- `handle(&PushRequest, now) -> PushResponse` takes each shard's records and answers the unwanted ones.

## Acceptance criteria

- AC-1: a shard no engine tracks is unwanted; a tracked shard keeps records above its checkpoint under consecutive ordinals and skips the rest.
- AC-2: at 100,000 held records the next record makes the shard refuse and answer unwanted, and `track` clears that, drops records at or below the new checkpoint and keeps ordinals going.
- AC-3: after the last engine untracks, pushes are taken for 30 s and unwanted after, and an expired shard is let go on the next untrack of any shard.
- AC-4: `from` gives held records at or after an ordinal, in order; `next_ordinal` is the next to be assigned.
- AC-5: `answered` with no subscription clears only the granting writer's grant; with a new subscription it replaces the grant (complete only if the answer was complete and its records were taken) and starts it at the send time plus 500 ms; a renewal extends it and never shortens it and keeps `complete`.
- AC-6: `granted` is false for each missing condition (no engine, refusing, skipped, incomplete, another writer, no subscription, run out) and true when all hold.
- AC-7: `handle` answers exactly the shards whose records were not taken.

## Test plan

`crates/node/tests/tail_held.rs`; one test per criterion, with the bounds asserted at the real constants.

## S3 request budget

Zero: memory only.

## Bounds

100,000 records a shard, freed at `track`, and a shard nobody tracks is dropped 30 s after its last engine closed. There is no node-wide total here; the node assembly's open-shard cap bounds the shards.

## Switch

None yet: nothing calls it until the engine and handler are wired; the overlay setting arrives with the wiring.

## Risks

Serving a grant after the writer stopped pushing returns stale reads: the grant is short (500 ms) and every condition above is tested.
