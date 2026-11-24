# Verified: tenant tier budgets

- AC-1: `a_create_keeps_its_group_tier_and_budget_and_refuses_a_bad_tier_or_budget` in crates/node/tests/server/index_route.rs
- AC-2: `billing_settings_are_written_and_read_back_from_descriptor_settings` in crates/node/tests/tenants.rs
- AC-3: `a_tenant_is_judged_against_the_largest_budget_of_its_indices` in crates/node/tests/tenants.rs
- AC-4: `a_tenant_is_judged_against_the_largest_budget_of_its_indices` in crates/node/tests/tenants.rs
- AC-5: `the_route_reports_the_mode_and_each_tenant` in crates/node/tests/tenants.rs
- AC-6: `tenant_tier_budgets_default_to_twenty_two_hundred_and_two_thousand` in crates/node/src/boot/config/tests.rs
