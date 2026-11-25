# Verified: restore-index-settings

- AC-1: `crates/coordinator/tests/restore_settings.rs`, `index_settings_are_stored_on_the_restored_index`, `a_bad_value_is_400_and_an_unknown_key_501`, and `a_stock_restore_applies_index_settings`.
- AC-2: `crates/coordinator/tests/restore_settings.rs`.
- AC-3: `crates/coordinator/tests/restore_settings.rs`, `a_fixed_setting_is_refused_before_anything_is_written`.
- AC-4: `a_restore_costs_what_the_budget_says`, unchanged and passing.
- AC-5: `a_request_with_neither_key_costs_what_one_with_empty_keys_costs`.

The tests passed on their first run: none was seen red before the code existed.
