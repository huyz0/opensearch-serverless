# The coordinator walks a delete- or update-by-query one shard at a time

## Serves

Parity plan R7.3, document APIs, fifth piece: the walk of the Java shell's `DeleteByQueryExecution` (ops package, shell 429b4cbc), which `UpdateByQueryExecution` reuses with a different change. The search for each page, the write of each change and the memory admission are the caller's, behind two traits; the node assembly adapts them.

## Change

`oss-coordinator` gains `docs::by_query`: `walk`, which visits the matching documents of each shard in sequence-number order, a page at a time, applying a change to each, and gives the `Walk` counts; with the `Source` (the newest matching sequence number of a shard, a page of hits, admission of a page's bytes) and `Change` (apply to one hit) traits.

## Acceptance criteria

- AC-1: shards are walked in the order given; for each, the newest matching sequence number is asked first, and a shard with no match is skipped without a page being asked.
- AC-2: pages of a shard are asked ascending after the previous page's last sequence number (none for the first), bounded above by the newest number seen at the start (so documents written during the walk are not chased), and as large as the smaller of 1,000 and the documents still allowed; a page that comes back empty ends the shard.
- AC-3: the walk stops once `max_docs` documents were visited, across shards: later shards are not asked; a page that is short or reaches `max_docs` is the shard's last.
- AC-4: each visited hit is changed; the walk counts those changed, those the change left alone (noops) and those that conflicted, and `batches` counts non-empty pages.
- AC-5: a conflict is counted and the walk goes on, unless it aborts on conflict: then the walk ends at once with the conflicting hit's id and the conflict's message, the hit counted as visited.
- AC-6: a refresh, when asked for, rides only the last hit of a shard's last page, not every change.
- AC-7: each page is admitted for the sum over its hits of 64 plus the id's length, in bytes, and the admission is held until the page's changes are done; a refused admission, a failed page and a failed change end the walk with that error.

## Test plan

`crates/coordinator/tests/by_query.rs`: an in-memory source of sorted shards that records every ask, and a change that records every call; counts and asks asserted exactly.

## S3 request budget

Zero in this piece: the pages and writes it asks for are metered where they are made; the walk asks one newest-number search for each shard and one search for each page of at most 1,000 documents, never one for each document.

## Bounds

One page of at most 1,000 hits at a time, admitted before its changes run.

## Switch

None: the shell's walk has no setting and no measured downside.

## Risks

A shard that keeps receiving matching writes cannot make the walk run forever: the upper bound is fixed at the start of that shard's walk.
