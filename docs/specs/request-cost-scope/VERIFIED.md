# Verified: request cost scope

- AC-1: `a_scope_counts_its_requests_by_kind_and_leaves_the_ledger_as_it_was` in crates/blob/tests/emulator/request_cost.rs (seen failing with the tap not adding to the scope)
- AC-2: `a_thread_with_no_scope_counts_nowhere` in crates/blob/tests/emulator/request_cost.rs
- AC-3: `a_scope_entered_on_a_spawned_thread_counts_into_the_same_counters` in crates/blob/tests/emulator/request_cost.rs
- AC-4: `a_guard_restores_the_scope_it_replaced_even_on_a_panic` in crates/blob/tests/emulator/request_cost.rs
- AC-5: `a_suspended_scope_counts_nothing_until_it_is_back` in crates/blob/tests/emulator/request_cost.rs
- AC-6: `a_share_rounds_to_the_nearest_whole_request` in crates/blob/src/cost.rs, and `the_price_is_the_ledgers` in crates/blob/tests/emulator/request_cost.rs
