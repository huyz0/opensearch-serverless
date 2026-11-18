# The builder fetches what a writer acknowledged when a shard opens

## Serves

Parity plan R7.6, the tail overlay engine, eighth piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), Accepted). Builds on [node-overlay-builder](../node-overlay-builder/SPEC.md) and [node-tail-links](../node-tail-links/SPEC.md). Registering the handlers and installing the builder under the setting are the next piece.

## Change

- `oss-reader`: `Stage::writer()` (the writer that staged the commit, if any) and `CommitAccess::index_name()`.
- `oss-node`: the `TailFetcher` trait (`fetch(writer, request)`), implemented by `TransportTailFetch`, and `OverlayBuilder::fetching`, a builder that holds one. On the first ask for a shard at a stage that names a writer, the builder queues a fetch with its build: the worker asks that writer for the records past the commit's checkpoint, subscribing this node (`NEW`), gives the answer to `TailHeld::answered` (which holds its records and records the grant), and builds over what is held. Without a fetcher the builder is as before.
- The ask is made once for a shard at a stage; a new stage asks again. A stage that names no writer, a writer that does not answer and a node with no fetcher ask nothing, and the shard waits for the writer's pushes.

## Acceptance criteria

- AC-1: the first ask for a shard at a stage naming a writer makes one fetch of that writer with the index name, the uuid, shard 0, the commit's checkpoint as `after_seq_no`, this node as the subscriber and `NEW`; the records it answers are held and the overlay built from them is returned once finished, without the caller pushing anything.
- AC-2: the answer's grant is recorded: with a complete answer `TailHeld::granted` holds for that writer; an incomplete one does not grant.
- AC-3: further asks make no further fetch; a new stage fetches again, after the old one is untracked and the shard tracked at the new commit's checkpoint.
- AC-4: a stage naming no writer makes no fetch; a fetch that is not answered holds nothing, is not repeated for the same stage, and writes pushed afterwards are still built.
- AC-6: a fetch queued for a stage the shard has since left, or closed, asks nobody and grants nothing.
- AC-5: `Stage::writer` gives the writer a commit's stage names and nothing for an unstaged commit.

## Test plan

`crates/node/tests/overlay_fetch.rs` with a counting fetcher over a commit a writer published; `crates/reader/tests/` for AC-5 in the existing stage tests.

## S3 request budget

Zero: a fetch is one transport request to a node, once per shard open, never to the object store; an idle shard makes none.

## Bounds

One fetch an open, on the worker, with the transport's 500 ms deadline; the answer's records are held under `TailHeld`'s caps. The worker is one thread, so an unanswered fetch holds it for that deadline: with a dead writer and a full queue of 64 fetches other shards' builds wait about 32 s, and serve the commit meanwhile.

## Switch

Inert until installed under `serverless.search.tail_overlay.enabled` (default `false`), which the wiring piece does.

## Risks

A search that opens the shard waits for no fetch: it serves the commit until the worker has the answer (within the deadline), so freshness is never older than the commit.
