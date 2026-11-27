# A search is not refused for a publish that landed since its shard opened

## Serves

Freshness: a count made straight after a refresh answers, instead of 503 `shard_behind_exception`. Follows [refresh-large-tail](../refresh-large-tail/SPEC.md); the bucket takeover cost test also polls for this 503.

## Change

A refreshed write the wait confirmed is in the writer's tail, not in a commit. A reader open on commit N asks the writer for what follows N; if a lazy publish has meanwhile made N+1 and the tail dropped what it holds, the answer is empty and incomplete, so the reader refuses as behind and lets the shard go. The next search opened it afresh and succeeded, but this one was answered 503.

- `SearchService::fresh_held`: on that refusal the shard is opened afresh on the newest commit and checked again, up to `REOPENS` (3) times, in the same search. The open is the one the next search would make; a shard still behind after them answers the refusal as before.
- The lazy publish pass stages the new commit for readers before the tail stops keeping what it holds, so a reader that opens in between finds the commit or the tail.

- Root cause of the CI flake (a 503 after all `REOPENS` on round 3): the writer keeps at most 16 MiB of unpublished records a shard. Once a shard's unpublished tail outgrows that, the oldest are dropped for room, and a reader whose commit is older than a dropped record is given no grant and no records, so it refuses; reopening cannot help because no commit holds the dropped records until a publish. A refresh confirmed by the readers' acks returned without publishing, leaving the shard in that state until the next lazy publish. Reopens are instantaneous, so they never outwait it. `await_visible` now answers false while a dropped record is not in a commit, so the refresh publishes (bytes-proportional, only past the bound), and the refresh's own register write lets the tail go (`published`) so the next refresh is not made to publish again.

- Second cause, found when CI failed AC-2 (about one run in seven): a search that applies the tail builds the shard's overlay itself while the overlay worker builds the same one. Each reserves the build's estimate from the `open_indexes` pool; the second is refused, the shard is marked skipped (it holds no grant), and every reopen meets the same pair. The builds of one shard now run one at a time, and the second finds the first's overlay covering the records and builds nothing, so the pool holds one estimate, not two.

Not changed: the refusal itself, the wire, any default.

## Acceptance criteria

- AC-1: three bulks of five 3 MiB documents, each followed by a refresh and a count, every count answers 200 with all documents so far.

- AC-2: three bulks of five 3 MiB documents on an index whose lazy publish is not due (`refresh_interval` 1h), each followed by a refresh and a count: every count answers 200 with all documents so far.

- AC-3: a search that builds an overlay for itself while the worker holds a queued build of the same shard leaves the worker nothing to build (one build, not two).

## Test plan

`crates/node/tests/refresh_large_tail.rs`, `a_count_after_a_refresh_is_answered_when_a_publish_landed_since_the_shard_opened`: red three runs of three before the change (503 on the second round), green six of six after. AC-2 is the deterministic form of the CI flake: red before the change on round 2 with the same 503 body, green after. Debug build, one machine, fs store: provisional.

## S3 request budget

Unchanged in steady state. A refused search now makes the open (the descriptor GET, or the register's two) that the next search would have made, in place of a failed answer and the client's retry; at most three, only on the refusal. No LIST, no new PUT.

## Switch

None: the alternative is a 503 for a state the node mends by itself.

## Risks

A shard that stays behind (a writer that publishes faster than the reopens) still answers 503 after three tries.
