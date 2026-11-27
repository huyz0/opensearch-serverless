# Verified: the commit's checkpoint and the index's mapping source

- AC-1: `a_commit_gives_the_checkpoint_its_writer_stored` in crates/reader/tests/it/overlay_checkpoint.rs
- AC-2: `a_commit_without_a_number_gives_minus_one` in crates/reader/tests/it/overlay_checkpoint.rs
- AC-3: `the_mapping_source_is_kept_for_the_overlay_s_builder` in crates/reader/tests/it/open.rs
- AC-4: `the_estimate_bounds_what_an_open_retains` in crates/reader/tests/open_cost_heap.rs
