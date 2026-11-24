# Verified

Numbers are provisional: one run each, a shared cloud container, release build, one reader, in-process sink for the push (no network), resident memory read from `/proc/self/statm`.

```bash
cargo test --release -p oss-node --all-features --test tail the_writers_tail_machinery -- --ignored --nocapture
```

- AC-1: `the_writers_tail_machinery_at_thousands_of_shards` in crates/node/tests/tail/tail_scale.rs. Memory is flat per shard: about 2.4 to 3.1 KB a shard with one subscription and one 200-byte record kept (11 to 48 MB at 5,000 to 20,000 shards); the keep's own bytes are 185 B a shard.
- AC-2: `the_writers_tail_machinery_at_thousands_of_shards` again, before and after the fix. Before: 1.3, 2.7, 5.9 and 74 us an acknowledgement and flushes of 0.15, 3.2, 32 and 1,407 ms at 100, 1,000, 5,000 and 20,000 shards. After: 1.0, 1.1, 1.4 and 1.7 us and flushes of 0.13, 1.5, 11 and 61 ms. The acknowledgement holds the queue's lock, so the old 74 us at 20,000 shards stalled every other acknowledgement.
- AC-3: `a_flush_starts_the_shards_count_over` in crates/node/tests/tail/tail_push.rs.
- AC-4: `cargo test -p oss-node --all-features --test tail`: 86 passed, 1 ignored (the evidence run).

Not measured: the reader's side (`TailHeld` and the overlay builder charging `open_indexes`) at thousands of shards, the network, and a run longer than one flush. The flush timer is one a node, not one a shard: the evidence run flushes once however many shards acknowledged.

Reader side, `the_readers_overlay_at_thousands_of_shards` in crates/node/tests/overlay_scale.rs (release, one run each, provisional; one shared 3-document commit for every shard, so a real commit's mask costs more; 20 records of about 20 bytes a shard; one builder worker):

```bash
cargo test --release -p oss-node --all-features --test overlay_scale -- --ignored --nocapture
```

| shards | pool | overlays built | charged | resident growth | time to build all |
|---|---|---|---|---|---|
| 100 | 256 MiB | 100 | 32.9 MB | 3.7 MB | 47 ms |
| 1,000 | 256 MiB | 796 | 262 MB | 19 MB | 407 ms |
| 5,000 | 256 MiB | 796 | 262 MB | 22 MB | 559 ms |
| 5,000 | 4 MiB | 12 | 3.9 MB | 0.6 MB | 288 ms |

- AC-5: `the_readers_overlay_at_thousands_of_shards`; the pool is never overdrawn (asserted). The held records cost about 4.4 to 5.5 KB a shard for 20 records. Each overlay is charged about 330 KB (`FIXED_BYTES` 256 KiB plus 4 KiB a record plus 4 times the bytes, an upper bound at the build's peak) and holds that charge while it lives, but grows the process by about 20 to 35 KB: the charge is ten to fifteen times what an overlay of a few records keeps. So the pool, not memory, limits how many shards have an overlay: a 256 MiB pool covers about 800 shards, and at 5,000 shards the other 4,200 get none.
- AC-6: NOT-RUN: that a refused build leaves the shard served from its commit is the builder's own test (`a_pool_that_refuses_builds_nothing_and_a_finished_overlay_holds_its_estimate` in crates/node/tests/overlay_builder.rs); what a search then does under each `refresh_visibility` was not run at this scale.

What it shows for the default: nothing in the reader's memory grows without the pool bounding it, but with the tail overlay on, a node needs about 330 KB of `open_indexes` for every shard it follows with any pushed write, or the shards past the pool lose the overlay's freshness. The charge could shrink to the overlay's measured size once built (`Reservation::shrink` exists); that is a change to the pool's accounting and its own piece.
