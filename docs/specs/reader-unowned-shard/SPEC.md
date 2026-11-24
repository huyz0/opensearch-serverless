# A reader serves a shard whose writer let it go and gave its head back

## Serves

[reader-tail-fresh](../reader-tail-fresh/SPEC.md) and [writer-shard-cap](../writer-shard-cap/SPEC.md). A reader asks the writer a commit names whether it holds every write answered as refreshed. A node that let the shard go (cap eviction, hand-off) answers "not this node's to say", the reader refuses with `shard_behind_exception`, and since the commit still names that writer and no write follows, every search of a cold evicted shard refuses without end. Measured over three real nodes with S3 latency (`hand_off_evidence`): with a cap, a `match_all` on an index whose shards were evicted failed every try for 60 s; with no cap it answered in 40 ms.

## Change

- `TailFresh::with_unowned(read)`: when a writer answers that it does not hold the shard (`claimed_through` is the maximum) and the commit is behind that claim, the reader reads the shard head once (verdict kept for 2 s per shard, at most 4,096 kept). A head that exists with no owner means the writer published everything before it let go, so the commit is all there is, and the search is served.
- A head owned by a node, absent, or unreadable keeps the refusal, so a shard taken by another writer, whose refreshed writes may sit only in its tail, is never served stale.
- The boot wiring reads heads through a `ShardHeadStore` on the node's store.

## Acceptance criteria

- AC-1: a writer answering "not mine to say" and a released head: the search is served.
- AC-2: the same answer and a head with an owner: the search is refused, and counted as a refusal.
- AC-3: the head is read once for searches within 2 s of each other.
- AC-4: with no head reader wired, the refusal is as before.
- AC-5: over three real nodes with a cap, a search on an evicted shard answers with every document (by hand, `--ignored`).

## Test plan

`crates/node/tests/tail/tail_fresh.rs` (AC-1 to AC-4); `crates/node/tests/hand_off_evidence.rs` (AC-5).

## S3 request budget

One head GET a search of an unowned shard at most every 2 s a shard, and only when a writer says it does not hold the shard and the commit is behind its claim. No LIST. Nothing for a shard whose writer answers.

## Switch

None: it only turns a refusal that never ends into an answer, with no cost anywhere else.

## Risks

- A head read for 2 s may name a node that has since taken the shard: the reader serves the commit for up to 2 s while the new owner's refreshed writes sit in its tail. A refreshed write acknowledged in that window can be missed by that search.
- Hibernation without give-back (`give_back_after_millis` 0, the default) keeps the head with the sleeping node, so a reader still refuses its shards; unchanged by this and not fixed here.
