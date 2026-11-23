# Verified: node-index-cache

- AC-1: `an_index_read_once_is_served_from_memory_for_a_window` in crates/node/tests/roles/index_cache.rs
- AC-2: `after_a_window_an_unchanged_index_costs_one_stripe_read_however_many_indexes_share_it` in crates/node/tests/roles/index_cache.rs
- AC-3: `an_update_is_seen_within_a_window_and_not_before` in crates/node/tests/roles/index_cache.rs
- AC-4: `a_delete_is_seen_within_a_window_and_a_recreate_after_it` in crates/node/tests/roles/index_cache.rs
- AC-5: `an_absent_name_is_never_kept` in crates/node/tests/roles/index_cache.rs
- AC-6: `an_index_another_stripe_holds_is_unaffected_by_a_change_elsewhere` in crates/node/tests/roles/index_cache.rs
