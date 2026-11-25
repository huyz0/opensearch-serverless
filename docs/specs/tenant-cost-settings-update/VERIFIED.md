# Verified: changing a tenant's cost tier or budget on a live index

- AC-1: `a_tier_is_one_of_three_and_a_budget_a_positive_whole_number` in crates/node/src/settings_route.rs, and `a_settings_update_moves_a_tenant_to_another_budget_at_once` in crates/node/tests/server/boot_usage.rs
- AC-2: `a_settings_update_moves_a_tenant_to_another_budget_at_once` in crates/node/tests/server/boot_usage.rs
- AC-3: `a_settings_update_moves_a_tenant_to_another_budget_at_once` in crates/node/tests/server/boot_usage.rs
