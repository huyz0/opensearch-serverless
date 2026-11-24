# Verified: tenant levels wiring

- AC-1: `the_indices_of_a_group_are_one_tenant` in crates/node/tests/tenants.rs
- AC-2: `a_tenant_over_its_budget_for_a_minute_is_at_level_one` in crates/node/tests/tenants.rs
- AC-3: `a_group_that_is_the_index_or_empty_is_not_remembered` in crates/node/tests/tenants.rs
- AC-4: `with_levels_off_nothing_is_judged` in crates/node/tests/tenants.rs
- AC-5: `the_route_reports_the_mode_and_each_tenant` in crates/node/tests/tenants.rs
- AC-6: `tenant_levels_observe_with_a_large_budget_unless_set` in crates/node/src/boot/config/tests.rs
- AC-7: `the_ticker_steps_the_levels_until_the_ledger_is_gone` in crates/node/tests/tenants.rs
