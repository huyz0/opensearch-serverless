# Verified

```bash
cargo test -p oss-reader --test overlay_heap -- --test-threads=1
cargo test -p oss-node --all-features --test overlay_builder
cargo test -p oss-node --all-features --test search_fresh
cargo test --release -p oss-node --all-features --test overlay_scale -- --ignored --nocapture
```

- AC-1: `the_kept_estimate_bounds_what_a_built_overlay_retains` in crates/reader/tests/overlay_heap.rs. Its first version, with a per-byte guess from the records alone, failed on the random and wide shapes (retained 267 KB, 445 KB and 2.5 MB against estimates of 234 KB, 130 KB and 1.4 MB); the estimate on the finished segment's size passes all ten shapes, at 1.2 to 5 times what is retained for the small ones and up to the build's estimate for random wide ones.
- AC-2: `a_finished_overlay_frees_its_peak_so_the_next_shard_builds_in_the_same_pool` and `an_overlay_is_reserved_its_estimate_and_the_masks_of_the_commit_to_build` in crates/node/tests/overlay_builder.rs. With the shrink commented out both fail.
- AC-3: `a_shard_with_no_overlay_answers_under_owner_and_serving_and_is_refused_under_everywhere` in crates/node/tests/search_fresh.rs. It characterises behaviour that was already there (the code reads grants, then applies), so it passed on its first run.
- AC-4: `the_readers_overlay_at_thousands_of_shards` in crates/node/tests/overlay_scale.rs: 3,518 overlays at 5,000 shards in 256 MiB, against 796; the pool is never overdrawn (asserted).

Not covered: a real commit's per-segment cost at thousands of different shards (the run shares one three-document commit), and the overlay's heap with a dynamic mapping that grows during the build.
