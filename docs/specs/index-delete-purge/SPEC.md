# Index delete: a purge that does not hold a request, and one that outlasts a pin

## Serves

Follow-up to [index delete](../index-delete/SPEC.md) and [ADR 0012](../../adr/0012-index-deletes-and-reclaim.md)'s cost note on the purge's round trips.

## Change

- `oss-meta` purge deletes a shard's listed page with `delete_many` (one batch request per 1,000 blobs on S3 and the emulator; a store with no batch sends one a blob). Wins everywhere, so no setting.
- `IndexLifecycle::with_inline_purge_limit(n)`: an index of more than `n` shards files its intent as `had_writer` and is purged at the reclaim, not inside the delete. Default no limit (the Java).
- `IndexLifecycle::with_repurge_pinned(true)` and `reclaim_at(intent, now)`: a purge that leaves pinned shards files the intent again, so they are purged once the pin ends. Default off (the Java).
- The node turns both on: `Config::inline_purge_max_shards` (16) and re-purge.
- Not changed: `reclaim`, the intent format, bucket ownership, snapshot pins.

## Acceptance criteria

- AC-1: a shard's blobs are deleted by one request per 1,000 blobs (emulator, S3), by one a blob where the store has no batch delete.
- AC-2: with a limit, an index above it is not purged by its delete, its intent says `had_writer`, and its reclaim purges every shard.
- AC-3: an index at or under the limit is purged inside its delete.
- AC-4: with re-purge on, a shard a pin names is left, the intent is filed again, and a reclaim after the pin ends purges it and files nothing more.
- AC-5: with re-purge off, a pinned shard's intent is not filed again.
- AC-6: through the node, a delete above `inline_purge_max_shards` leaves its bytes until the drain, and a pinned shard goes at the first drain after its view expires.

## Test plan

AC-1..5: `crates/meta/tests/descriptors/reclaim_modes.rs` on emulator, S3 emulation and filesystem. AC-6: `crates/node/tests/index_delete.rs`; seen failing while the drain called `reclaim` and not `reclaim_at`.

## S3 request budget

Delete of a no-writer index within the limit: unchanged for one blob a shard; fewer DELETEs for more (1,001 blobs: 3 instead of 1,002). Above the limit: the request does the descriptor, intent and head requests only. A pinned deleted index costs the pins' read (one LIST page per 1,000 views, a GET each) each reclaim until its views expire, about one a few minutes: bounded by the views' keep-alive.

## Switch

Both are lifecycle settings, off in `oss-meta`'s defaults and on in the node, because their downside is real: deferral leaves bytes (billed) for the reclaim delay, and re-purge adds the pins' read until the pin ends.

## Risks

- The re-filed intent is read every drain tick until due; one LIST a tick regardless of how many.
- A deleted index pinned by a long keep-alive is re-checked every few minutes for that long.
