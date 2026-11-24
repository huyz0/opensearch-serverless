# A finished overlay is charged what it keeps

## Serves

[tail-overlay-scale](../tail-overlay-scale/SPEC.md) found that the overlay builder holds each finished overlay's build-peak estimate in the `open_indexes` pool (about 330 KB for 20 small records) while the overlay keeps about 30 KB. A 256 MiB pool then covers about 800 shards with an overlay and a node that follows thousands serves the rest from their commit. [ADR 0035](../../adr/0035-the-tail-overlay-is-on-by-default.md) lists it.

## Change

- `oss_reader::overlay::kept_bytes(records, stored)` estimates what a finished overlay keeps: a fixed 64 KiB, three times the bytes of its segment files (`Overlay::stored_bytes`, summed over its in-memory directory), and a quarter of the source it decoded, never more than the build's estimate.
- The builder still reserves the build estimate and the commit's masks before building, which bounds the peak. Once the overlay is built it gives back the difference (`Reservation::shrink`): the pool holds `kept_bytes` plus the masks while the overlay lives.
- Nothing else moves: a refused build, a rebuild, a closed shard and the one worker are as they were.

## Acceptance criteria

- AC-1: for every shape in `overlay_heap` (small and large records, compressible and random values, a wide mapping) `kept_bytes` is at least the heap the finished overlay retains, and at most the build's estimate.
- AC-2: a pool with room for one build's peak and half a peak more builds the overlays of two shards, because the first gives its peak back; and a finished overlay's charge is `kept_bytes` plus the masks.
- AC-3: a shard the pool left without an overlay is searched from its commit alone: under visibility `owner` and `serving` it answers and asks its writer nothing; under `everywhere` it asks its writer and is refused as behind when the writer claims writes its commit lacks.
- AC-4: at 5,000 shards in a 256 MiB pool the evidence run builds more overlays than before (796), and what it charges stays at or below the pool.

## S3 request budget

None: the pool is memory.

## Switch

None. The charge was an upper bound at the build's peak held for the overlay's life; the finished overlay's charge is now an upper bound on what it keeps, measured over the shapes above, and the peak is still reserved before the build. No downside was measured, so no switch.
