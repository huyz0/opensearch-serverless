# Verified: tenant write pacing

- AC-1: `documents_written_are_counted_per_tenant_and_reported` in crates/node/tests/tenants.rs
- AC-2: `an_enforced_tenant_at_level_three_is_held_to_its_budget_and_refused_past_the_longest_wait` in crates/node/tests/tenants.rs
- AC-3: `a_write_the_gate_holds_waits_before_it_is_applied` in crates/node/tests/roles/write_gate.rs
- AC-4: `a_group_the_write_gate_refuses_is_neither_applied_nor_logged_and_asks_for_a_retry` in crates/node/tests/roles/write_gate.rs
