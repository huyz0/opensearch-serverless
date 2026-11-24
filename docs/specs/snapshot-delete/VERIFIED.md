# Snapshot delete, shallow: verified

Run on 2026-11-24 in a Linux sandbox over the in-process emulator: `cargo test -p oss-node --all-features --test server snapshot_delete`.

- AC-1: `a_delete_releases_the_name_and_the_pin_and_the_name_can_be_taken_again`
- AC-2: `deleting_what_is_not_there_is_404_and_a_second_delete_too`
- AC-3: `a_selector_or_a_bad_name_is_refused_and_a_switch_off_is_501`
- AC-4: `a_snapshot_that_copied_its_data_is_refused_and_left_alone`
- AC-5: `a_take_still_in_progress_can_be_deleted`
- AC-6: `a_delete_costs_what_the_spec_budgets`
