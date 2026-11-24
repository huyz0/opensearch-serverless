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
