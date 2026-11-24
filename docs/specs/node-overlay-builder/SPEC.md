# The node builds overlays in the background

## Serves

Parity plan R7.6, the tail overlay engine, sixth piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), Accepted). Builds on [reader-overlay-source](../reader-overlay-source/SPEC.md) and [reader-overlay-checkpoint](../reader-overlay-checkpoint/SPEC.md). The pool charge for the cached view's masks, the fetch on open and the transport handlers are later pieces.

## Change

- `oss-reader`: `overlay::build_ready(commit, mapping_json, records, max_bytes) -> Result<Ready, BuildError>` takes the commit's checkpoint, the ids' copies in the commit and builds the overlay over them. The `OverlaySource` trait now hands a source an `Arc<dyn CommitAccess>` (the commit's reader and the index's mapping source) in place of a bare reader, so a background build can hold the commit it builds over, and gains `closed` (a shard's engine let go).
- `oss-node`: `overlay_builder::OverlayBuilder`, an `OverlaySource` over `TailHeld`. `ready` never blocks and never reads the store: it returns the finished `Ready` when it covers every record held, else queues one build on a worker thread through a bounded queue and returns the last finished `Ready` over the same commit, or nothing. The worker converts the held records, reserves their estimate from the `open_indexes` pool with a try-reserve that never evicts an open index, builds, and keeps the charge with the `Ready` until it is replaced or the shard closes.
- First sight of a shard at a stage tracks it in `TailHeld` at the commit's checkpoint; a new stage, or `closed` for the stage it holds (one for an older stage changes nothing), untracks it and drops its `Ready` and charge.

## Acceptance criteria

- AC-1: `ready` returns at once with nothing before a build finishes, and the build is queued once however many searches ask; after the build `ready` returns an overlay that shows the held records over the commit.
- AC-2: while the held records are unchanged `ready` returns the same `Ready` (pointer-equal) and builds nothing more (`builds()` stays).
- AC-3: new records held make one new build; until it finishes `ready` returns the previous `Ready` for the same commit.
- AC-4: a build whose estimate the pool refuses builds nothing, holds no charge and serves the commit; the pool's in-use bytes are what they were before. A finished overlay holds in the pool what it keeps (`kept_bytes`, see [tail-overlay-kept-charge](../tail-overlay-kept-charge/SPEC.md)), not its build estimate, until replaced or closed, and then returns it.
- AC-5: a build that fails (a record the mapping refuses) is not retried until the held records change; searches serve the commit. A build the pool refused is tried again once the pool has the refused bytes free, with no record changed.
- AC-9: a build queued for a stage the shard has since left is dropped with its charge, and the new stage keeps its own.
- AC-6: the first `ready` for a shard tracks it at the commit's local checkpoint (records at or below it are never held); a different stage tracks it again and drops the old `Ready`; `closed` for its stage untracks it, drops its `Ready` and returns its charge; `closed` for an older stage changes nothing.
- AC-7: the queue is bounded: with the worker paused and a queue of one, three shards' asks queue at most two builds and return at once; after the worker resumes every shard's overlay is finished by later asks. `pause` and `resume` hold the worker (shutdown uses them too).
- AC-8 (`oss-reader`): `build_ready` leaves out the records the commit already holds (checkpoint and copies) and keeps newer ones; `local_checkpoint` and `mapping_json` are those of the commit and index it is given.
- AC-10: an overlay is reserved its estimate plus the masks to build its cached view holds (`mask_bytes`: one bit set of `max_doc` bits for each commit segment); a pool with room for the estimate alone refuses, and one with room for both accepts.

## Test plan

`crates/node/tests/overlay_builder.rs` over a commit a writer published to a file store, pushing records into a real `TailHeld`, with `settle()` (waits for the worker to be idle) and `builds()` on the builder. `crates/reader/tests/overlay_build_ready.rs` for AC-8.

## S3 request budget

Zero: the builder is given no store. The worker reads the open commit through the shard's directory and cache, as a search does, and the records come from memory.

## Bounds

Records held are capped per shard by `TailHeld`; one build holds at most `MAX_IDS` ids and its estimate is reserved first. Queue depth is a constant; one worker.

## Switch

`tail_overlay::install` installs a source only under `serverless.search.tail_overlay.enabled` (default `false`); nothing constructs a builder yet, so the node wiring piece adds the both-values test.

## Risks

A build over another commit would hide the wrong documents: the job holds the commit it was queued with and the `Ready` is kept per stage. A stalled worker leaves searches on the commit, never blocked, but up to the queue's length of engines stay pinned by their queued jobs. The worker copies the held records before it reserves: they are already in memory, capped per shard by `TailHeld`. A slot is keyed by stage, so an engine evicted and reopened on the same commit that closes late drops the new engine's overlay once; the next ask rebuilds it.
