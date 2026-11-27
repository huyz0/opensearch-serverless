# Pack first search: verification

Emulator with the Java soak's mean request latency (20 ms and 7 ms jitter), release build, one machine: counts are exact for the run, times provisional. Run by hand: `cargo test -p oss-node --features writer --release --test first_search_packs -- --ignored --nocapture`. The time bound was seen failing first (tightened to 0.5: packed 4.472 s against blob 4.480 s).

- AC-1: `the_first_open_of_a_thousand_packed_shards_costs_what_the_blob_open_does` (node, `tests/first_search_packs.rs`): 1,000 shards, 4,000 documents in each placement, no LIST.
- AC-2: `the_first_open_of_a_thousand_packed_shards_costs_what_the_blob_open_does` (node, `tests/first_search_packs.rs`): blob 4.473 s, packed 4.476 s (1.001 times).
- AC-3: `the_first_open_of_a_thousand_packed_shards_costs_what_the_blob_open_does` (node, `tests/first_search_packs.rs`): blob 3,000 requests, packed 3,000 (three an open in both).
