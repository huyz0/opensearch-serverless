# The tail overlay at thousands of shards

## Serves

[ADR 0035](../../adr/0035-the-tail-overlay-is-on-by-default.md) left the writer's memory and timers across many shards unmeasured ("no run with thousands of shards has been made"). This measures the writer's side and the reader's side. Writer: [writer-tail-keep](../writer-tail-keep/SPEC.md), [writer-tail-push](../writer-tail-push/SPEC.md) and the subscriptions in `TailServe`, with one reader following every shard.

## Change

- An evidence test (`crates/node/tests/tail/tail_scale.rs`, run by hand) holds 100 to 20,000 shards, each acknowledging one 200-byte write in the same flush interval, then flushes once; it prints resident memory a shard, the bytes kept, the time to acknowledge all and the time to flush.
- The first run found the queue quadratic in shards: `TailPush::acked` summed the whole queue to check its shard's bound, and `flush` looked up each record's shard by scanning the shards grouped so far. `TailPush` now keeps the records queued a shard in a map and groups a flush through a map. Behaviour is unchanged.

- Reader side: an evidence test (`crates/node/tests/overlay_scale.rs`, run by hand) tracks 100 to 5,000 shards on one `OverlayBuilder`, holds 20 pushed records for each, and asks for every shard's overlay until each is built or refused; it prints memory held, overlays built, the `open_indexes` bytes charged, resident memory and the time to build them all with the one worker. No behaviour changes.

## Acceptance criteria

- AC-1: the evidence run reports memory, bytes kept and times at 100, 1,000, 5,000 and 20,000 shards, and every write acknowledged is kept.
- AC-2: acknowledging one write on each of 20,000 shards costs the same per acknowledgement as on 1,000 within a factor of 3 (it was 27 times), and its flush is linear (it was quadratic).
- AC-3: the shard's queue bound counts what is queued since the last flush, not what was ever queued.
- AC-4: every `writer-tail-push` criterion still holds.
- AC-5: the reader-side run reports, at 100, 1,000 and 5,000 shards with a 256 MiB pool and at 5,000 with 4 MiB, the overlays built, the bytes charged and the resident growth; the pool is never overdrawn.
- AC-6: a shard whose overlay the pool refused is served and answers as the code says (served from its commit; a search that needs the tail is refused as behind). Not run: see VERIFIED.

## S3 request budget

None: the tail is memory and the transport; no object-store request is made.

## Switch

None. A performance fix with unchanged behaviour, and a measurement.
