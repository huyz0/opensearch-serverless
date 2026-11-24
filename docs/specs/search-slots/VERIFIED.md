# Verified: search slots

- AC-1: `a_permit_is_taken_and_given_back_and_the_tenant_entry_goes` in crates/node/tests/search_slots.rs
- AC-2: `a_gate_that_is_not_full_admits_at_once_and_a_full_one_lets_a_tenant_within_its_share_run` in crates/node/tests/search_slots.rs
- AC-3: `a_tenant_over_its_share_waits_on_a_full_gate_and_a_quiet_one_does_not` in crates/node/tests/search_slots.rs
- AC-4: `a_wait_ends_at_its_deadline_and_the_search_runs_as_an_overflow_runner` in crates/node/tests/search_slots.rs
- AC-5: `past_the_waiter_cap_an_over_share_search_runs_at_once` in crates/node/tests/search_slots.rs
- AC-6: `a_level_two_tenant_waits_at_half_the_slots_only_with_another_tenant_active` in crates/node/tests/search_slots.rs
- AC-7: `a_tenant_over_its_share_waits_on_a_full_gate_and_a_quiet_one_does_not` in crates/node/tests/search_slots.rs
- AC-8: `the_report_counts_what_happened` in crates/node/tests/search_slots.rs
- AC-9: `the_state_holds_one_entry_a_tenant_and_no_more` in crates/node/tests/search_slots.rs
- AC-10: `the_settings_default_to_off_and_refuse_what_leaves_no_headroom` in crates/node/tests/server/boot_search_slots.rs
- AC-11: `a_node_with_slots_serves_searches_and_reports_the_gate` in crates/node/tests/server/boot_search_slots.rs
