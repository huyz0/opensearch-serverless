# The overlay builder applies pending writes before a search that needs them

## Serves

Parity plan R9.3 (refresh semantics). A reader that holds a grant from its writer ([reader-tail-fresh](../reader-tail-fresh/SPEC.md)) holds every write the writer answered as refreshed, but the overlay a search serves is built on a worker and may still lag the records held. The shell's `applyPending` has the engine take what is held before the search runs; this is the same step for the builder, so a granted shard serves what it holds.

## Change

- `oss-reader`'s `OverlaySource` gains `apply_pending(index_uuid, shard, stage, commit)`, a no-op by default.
- `oss-node`'s `OverlayBuilder` implements it: when the shard is tracked at that stage, records are held and the finished overlay does not cover them, it builds in the caller, charged to the same pool, and returns. If the overlay still does not cover them (the pool has no room, the engine refuses a record) it marks the shard skipped (`TailHeld::skipped`), so the shard holds no grant until an engine opens anew.
- A shard nothing tracks, a different stage and a shard with no records are left alone. `ready` is unchanged: a search never starts a build.

Nothing calls it yet.

## Acceptance criteria

- AC-1: with records held and nothing built, `apply_pending` builds once before returning and `ready` then gives an overlay covering them; with nothing new held it builds nothing more; after another record it covers both; an untracked shard and another stage build nothing.
- AC-2: when the pool has no room for the overlay, `apply_pending` builds nothing and the shard, which held a grant, no longer holds one.

## Test plan

`crates/node/tests/overlay_builder.rs`: `applying_pending_builds_what_is_held_before_it_returns` and `a_record_that_cannot_be_applied_marks_the_shard_skipped_so_it_holds_no_grant`. Written first against a method that did not exist (red by compilation).

## S3 request budget

None: memory only. LIST 0, HEAD 0, PUT 0.

## Risks

- A build on the search thread costs that search the build (bounded by the held records, at most 100,000, and the pool's charge); it happens only when the overlay lags, and only once the visibility mode calls it.
