# Tenant levels: verification

- AC-1: `a_tenant_over_budget_climbs_a_level_a_minute_then_two_minutes_apart` (blob, `tests/guard.rs`).
- AC-2: `a_tenant_under_budget_stays_at_zero_and_one_that_falls_back_loses_a_level_in_five_minutes` (blob, `tests/guard.rs`).
- AC-3: `a_level_set_from_outside_holds_when_higher_and_bad_pairs_are_left_out` (blob, `tests/guard.rs`).
- AC-4: `a_refusal_shows_as_level_four_for_a_minute` (blob, `tests/guard.rs`).
- AC-5: `a_write_is_held_to_what_fits_the_budget_and_refused_past_the_longest_wait` (blob, `tests/guard.rs`).
- AC-6: `only_an_enforced_tenant_at_level_three_is_paced` (blob, `tests/guard.rs`).
- AC-7: `a_tenant_a_tick_does_not_name_is_forgotten` (blob, `tests/guard.rs`).
