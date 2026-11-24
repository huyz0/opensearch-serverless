# A node at its shard cap can clear room in bursts, and can pin indexes it never lets go of

## Serves

R13.2: the shell's `evictHeadroomFraction` and `BackgroundReconciler.pin`. [writer-shard-cap](../writer-shard-cap/SPEC.md) lets an idle shard go for each open at the cap, one victim search an open. Headroom lets one pass clear room for a burst; a pin keeps an index resident.

## Change

- `serverless.activation.evict_headroom_percent` (node scope, `OSS_EVICT_HEADROOM_PCT`, 0 to 100, **default 0**): making room goes on, after the shard it needs, to the cap less that percent of it (at least one shard), while idle shards remain. 0 lets go of one shard, as before.
- `serverless.activation.pinned_indexes` (node scope, `OSS_PINNED_INDEXES`, comma-separated index names, **default empty**): a pinned index's shards are never let go of here to make room, to hand off, or for idleness (hibernation). A node full of pinned shards refuses a new open, as one with no idle shard does.

## Acceptance criteria

- AC-1: with headroom 50% of a cap of 4 and four idle shards held, one open at the cap lets go of two shards, not one.
- AC-2: with headroom 0 the same open lets go of one shard.
- AC-3: headroom stops at the shards that can go: with one idle shard not pinned, one is let go however large the headroom.
- AC-4: a pinned index's shard is not let go of to make room even when it is the idlest; the next idlest goes.
- AC-5: a node whose idle shards are all pinned refuses the open.
- AC-6: a pinned index's idle shard does not hibernate; an unpinned one does.
- AC-7: both settings are registered with their defaults, and `OSS_EVICT_HEADROOM_PCT` and `OSS_PINNED_INDEXES` set them and refuse a headroom outside 0 to 100.

## Test plan

`crates/node/tests/writer_shard_cap.rs` (AC-1 to AC-6) over a role on the emulator; `crates/node/src/boot/config/tests.rs` and the settings registry test (AC-7).

## S3 request budget

Unchanged per shard let go (a publish when unpublished, a head read and a swap). Headroom lets go of more shards in one pass but never more than the opens it serves, plus at most the headroom share of the cap, once. A pin reads names from memory: no request.

## Switch

Both off by default. Headroom lets go of shards nobody asked for room for, which costs those requests if demand then stops; it flips on only when a run shows the saved victim searches matter.

## Risks

- A pin set on every node for many large indexes can fill a node: the cap refuses new opens, as designed.
- Pins are per node and read at start, as in the shell; changing them takes a restart.

## Deferred, by decision

- Write holds (the shell's `WriteHold`) serve only shard splits; they belong with the split milestone, which has no Rust mover yet.
- The cap stats (`ShardCapStats`: max, held, refused, evicted, handed off, steered) get a node-stats block when a node-stats route is built; there is none today.
