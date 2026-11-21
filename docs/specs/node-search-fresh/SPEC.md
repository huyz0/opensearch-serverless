# A search asks whether its shard holds every refreshed write before it answers

## Serves

Parity plan R9.3 (refresh semantics), search half. Builds on [reader-tail-fresh](../reader-tail-fresh/SPEC.md) (the decision), [node-overlay-apply](../node-overlay-apply/SPEC.md) (the engine takes what is held) and [writer-tail-await](../writer-tail-await/SPEC.md). Not here: the `serverless.search.refresh_visibility` setting that turns grants on, and the write route that waits.

## Change

`oss-node`:

- `SearchService::set_tail_fresh` installs a `TailFresh`. Every search of a shard (`_search`, `_count`, `_msearch`, a shard query a coordinator sends) calls it before it takes the overlay view.
- The shard's writer and commit checkpoint come from the commit it is open on. A shard with no staging writer, or whose writer is this node, is not asked about: it holds every write.
- A shard that may not answer is a 503 shard failure, `shard_behind_exception`, naming the index, shard and node.
- `TailParts::grants` (default off) is whether the installed check requires grants; `wire` installs the check with it. Off, nothing is asked and searches are as before.

## Acceptance criteria

- AC-1: a shard behind the claim its writer reports is refused as a 503 `shard_behind_exception` shard failure on `_search`.
- AC-2: the same refusal on `_count` and `_msearch`.
- AC-3: a shard at or above the claim answers.
- AC-4: a shard written by this node, and a check with grants off, ask nothing.

## Test plan

`crates/node/tests/search_fresh.rs` with a fetcher that answers every ask with a claim.

## S3 request budget

None: at most one round trip to the writer per search that holds no grant. LIST 0, HEAD 0, PUT 0.

## Risks

- Not yet reachable from a setting: `TailParts::grants` defaults off, so a node serves as before.
