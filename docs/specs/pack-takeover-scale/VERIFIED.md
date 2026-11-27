# Pack takeover scale: verification

Emulator with the Java soak's mean request latency (20 ms and 7 ms jitter), release build, one machine: counts are exact for the run, times provisional. Run by hand: `cargo test -p oss-node --features writer --release --test takeover_packs -- --ignored --nocapture`. The bound was seen failing first (ratio tightened to 0.5: packed 43.77 s against blob 43.47 s).

- AC-1: `a_dead_nodes_packed_shards_are_taken_within_a_tenth_of_its_blob_shards` (node, `tests/takeover_packs.rs`): 844 shards named by the log (4,000 documents published, 2,000 more in the log), 844 taken in each placement, no LIST, every shard's highest sequence number equal to the dead node's.
- AC-2: `a_dead_nodes_packed_shards_are_taken_within_a_tenth_of_its_blob_shards` (node, `tests/takeover_packs.rs`): blob 43.57 s, packed 43.62 s (1.001 times).
- AC-3: `a_dead_nodes_packed_shards_are_taken_within_a_tenth_of_its_blob_shards` (node, `tests/takeover_packs.rs`): blob 13,405 requests, packed 13,302.
