# A search is not refused for a publish that landed since its shard opened

## Serves

Freshness: a count made straight after a refresh answers, instead of 503 `shard_behind_exception`. Follows [refresh-large-tail](../refresh-large-tail/SPEC.md); the bucket takeover cost test also polls for this 503.

## Change

A refreshed write the wait confirmed is in the writer's tail, not in a commit. A reader open on commit N asks the writer for what follows N; if a lazy publish has meanwhile made N+1 and the tail dropped what it holds, the answer is empty and incomplete, so the reader refuses as behind and lets the shard go. The next search opened it afresh and succeeded, but this one was answered 503.

- `SearchService::fresh_held`: on that refusal the shard is opened afresh on the newest commit and checked again, up to `REOPENS` (3) times, in the same search. The open is the one the next search would make; a shard still behind after them answers the refusal as before.
- The lazy publish pass stages the new commit for readers before the tail stops keeping what it holds, so a reader that opens in between finds the commit or the tail.

Not changed: the refusal itself, the wire, any default.

## Acceptance criteria

- AC-1: three bulks of five 3 MiB documents, each followed by a refresh and a count, every count answers 200 with all documents so far.

## Test plan

`crates/node/tests/refresh_large_tail.rs`, `a_count_after_a_refresh_is_answered_when_a_publish_landed_since_the_shard_opened`: red three runs of three before the change (503 on the second round), green six of six after. Debug build, one machine, fs store: provisional.

## S3 request budget

Unchanged in steady state. A refused search now makes the open (the descriptor GET, or the register's two) that the next search would have made, in place of a failed answer and the client's retry; at most three, only on the refusal. No LIST, no new PUT.

## Switch

None: the alternative is a 503 for a state the node mends by itself.

## Risks

A shard that stays behind (a writer that publishes faster than the reopens) still answers 503 after three tries.
