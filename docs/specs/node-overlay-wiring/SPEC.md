# One call turns the tail overlay on for a node

## Serves

Parity plan R7.6, the tail overlay engine, last piece before node assembly ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), Accepted). Builds on [node-overlay-builder](../node-overlay-builder/SPEC.md), [node-overlay-fetch](../node-overlay-fetch/SPEC.md) and [node-tail-links](../node-tail-links/SPEC.md).

## Change

`oss-node`: `tail_overlay::wire(service, enabled, budget, fetcher, subscriber, clock)` builds the node's `TailHeld`, an `OverlayBuilder::fetching` over it, installs the builder on the search service and returns `Wired { held, push, builder }`, where `push` is the `TailPushHandler` the node's assembly registers on its transport. Off, or with a source installed already, it builds nothing and returns `None`.

## Acceptance criteria

- AC-1: with `serverless.search.tail_overlay.enabled` off (its default) `wire` returns `None`, installs nothing and a search is answered from the commit alone.
- AC-2: with it on, a search of a shard whose commit names a writer makes the builder ask that writer; the records it answers are held, the writer is granted, and the next search sees the overlay (a document the commit does not hold is counted and found by a term query), through the real search service and a real `OverlayBuilder`.
- AC-3: a second `wire` on a service that has a source installed returns `None` and installs nothing.

## Test plan

`crates/node/tests/overlay_search.rs` over a writer's published commit on a file store, a fetcher answering as the writer, and the node's HTTP search.

## S3 request budget

Zero: wiring builds in-memory structures; a fetch is a transport request, never an object-store call.

## Bounds

Those of the builder (one worker, 64 queued jobs, the `open_indexes` pool) and of `TailHeld`.

## Switch

`serverless.search.tail_overlay.enabled`, default `false`: until it is measured, nothing is built, no handler exists and a search reads the commit alone.

## Risks

Registering `Wired::push` on the transport and the writer-side handlers, and a fetcher over the live membership, belong to the node's assembly, which does not exist yet; until then the overlay is reachable only through this function.
