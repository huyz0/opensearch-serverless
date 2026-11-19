# Verified: the node decides each open's prefetch

- AC-1: `a_descriptors_settings_are_kept_as_their_bytes` in crates/format/tests/shell_blobs.rs
- AC-2: `a_descriptors_settings_are_read_into_the_index_meta` in crates/node/tests/it/open_prefetch.rs
- AC-3: `planned_bytes_are_the_ranges_lengths_gaps_included` in crates/reader/src/prefetch/plan_tests.rs
- AC-4: `the_setting_resolves_index_then_cluster_then_node_then_on` in crates/node/tests/it/open_prefetch.rs
- AC-5: `a_prefetching_open_holds_its_bytes_until_it_returns` and `a_pool_with_no_room_opens_without_prefetch` in crates/node/tests/it/open_prefetch.rs
- AC-6: `an_index_that_turns_it_off_holds_nothing` in crates/node/tests/it/open_prefetch.rs
