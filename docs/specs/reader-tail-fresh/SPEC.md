# A reader answers for a shard only while it holds every refreshed write

## Serves

Parity plan R9.3 (refresh semantics), reader half: the `ensureFresh` of the shell's `TailOverlays` in its `everywhere` visibility mode. Builds on [reader-tail-held](../reader-tail-held/SPEC.md), [writer-tail-fetch](../writer-tail-fetch/SPEC.md) (the writer's `CHECK` answer and claim) and [writer-tail-await](../writer-tail-await/SPEC.md), which waits out the grants this piece relies on. Not here: the search path calling it, the refusal that sends a search to the next copy, the `serverless.refresh_visibility` mode and the write route.

## Change

`oss-node` (feature `reader`):

- `TailHeld` remembers the subscription its grant is under, and gains `subscription_to_ask(uuid, shard, writer)`: the subscription held to renew, a new one (`NEW`) when none is held from that writer, or `CHECK` when no engine can use a subscription (no engine, a shard refusing for want of room, or an engine that left a record out).
- `tail_fresh::TailFresh::ensure_fresh(index, uuid, shard, writer, commit_checkpoint, apply_pending)` says whether the shard may answer a search that started before the call. With grants off it always may. Under a grant from the writer it may, after the engine took what is held. Without one it asks the writer once for every search that finds none meanwhile (a renewal, a subscription anew, or a `CHECK`), takes the answer's grant, and if it still holds none it may answer only when its commit's checkpoint is at or above the claim the writer reported. A writer that does not answer, or that this node's leases do not show live, is treated as dead for that search: it may answer from its commit (counted as unreachable), without a second ask.
- `counts()` gives the asks made, the refusals and the unreachable writers.

Nothing calls it yet.

## Acceptance criteria

- AC-1: with grants off nothing is asked and every search may answer.
- AC-2: under a grant a search asks nothing, and the engine takes what is held first.
- AC-3: without a grant the writer is asked for a subscription anew, then for a renewal of the one it holds once the grant has run out, and each answer's grant is taken.
- AC-4: a shard with no engine asks `CHECK` and takes no grant; its commit may answer only at or above the claim.
- AC-5: a writer that does not answer the grant ask is asked nothing more for that search, counted unreachable, and the search may answer; an answer to a second ask a moment later (a stale lease view refreshed in between) cannot refuse it.
- AC-6: an answer that leaves no grant (not complete) is compared with its claim without a second ask.
- AC-7: what is asked follows the engine and the subscription held (`CHECK`, `NEW`, the held id; another writer's grant is none; a skipped record or a refusing shard asks `CHECK`).

## Test plan

`crates/node/tests/tail_fresh.rs` with a scripted fetcher and a clock set by the test.

## S3 request budget

None: a round trip to the writer, at most one per search that holds no grant. LIST 0, HEAD 0, PUT 0.

## Risks

- An idle shard (searched less often than every 500 ms) asks the writer on each search: about one round trip, measured at +3 ms p50 in the shell. Not wired yet, so no cost today.
