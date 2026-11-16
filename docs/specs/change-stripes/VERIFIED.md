# Change stripes: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-format --test change_stripes`
- `cargo test -p oss-meta`
- the whole gate, `scripts/gate.sh`

The fixtures were written by running `java fixtures/change-stripes/Stripes.java fixtures/change-stripes` on OpenJDK 21.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| pending entries trimmed along with applied ones | `only_the_newest_applied_changes_are_kept_with_every_pending_delete` |
| a `Deleted` bump that does not finish the pending entry | `bumps_write_the_javas_bytes` |
| a delete that ignores a failed `PendingDelete` bump | `a_delete_whose_pending_entry_fails_is_refused` |

Evidence for each criterion:

- AC-1: `bumps_write_the_javas_bytes`, `a_delete_finishes_only_its_own_pending_entry`, `only_the_newest_applied_changes_are_kept_with_every_pending_delete`, `too_many_pending_deletes_start_a_new_epoch`, `what_the_java_writes_reads_back_and_writes_again`, `a_malformed_stripe_is_refused`, `the_longest_stripe_fits_its_bound`, `a_name_hashes_to_the_javas_stripe`, `each_stripe_has_its_key`
- AC-2: `a_change_to_another_index_in_the_stripe_leaves_an_index_confirmed`, `an_unfinished_delete_is_never_confirmed_until_it_finishes`, `an_update_does_not_finish_a_delete`, `a_read_older_than_every_kept_change_is_not_confirmed`, `too_many_unfinished_deletes_start_a_new_epoch`, `a_stripe_the_java_wrote_is_read`, `the_stripe_is_stored_as_the_javas_text`
- AC-3: `a_bump_is_a_read_and_a_swap`, `a_bump_retries_while_others_move_the_stripe`, `a_bad_name_or_stripe_sends_nothing`
- AC-4: `an_applied_update_is_recorded_and_a_refused_one_is_not`, `a_delete_is_recorded_pending_then_finished`, `a_delete_whose_pending_entry_fails_is_refused`, `a_delete_whose_finished_entry_fails_still_answers`, `a_failed_delete_is_still_finished_in_its_stripe`, `a_name_no_stripe_holds_is_not_recorded`, `each_operation_sends_what_the_spec_budgets`, unchanged, in `counts.rs`
