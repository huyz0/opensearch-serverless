# Alias mutations: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-format`
- `cargo test -p oss-meta`
- the whole gate, `scripts/gate.sh`

`cargo mutants` over the diff: every mutant is caught, after tests were added for the ones it first missed (the deadline, the reservation check, the hint fence, the deadline boundary and the wait). An independent review then found kept members read needlessly and several counts unasserted; both are fixed.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| publishing before preparing hints | `a_member_that_is_no_index_undoes_the_creation`, `a_failed_preparation_undoes_its_reservation`, `a_creation_removed_mid_preparation_is_superseded` |
| no check that the reservation holds | `a_reservation_taken_over_mid_preparation_is_superseded` |
| removed members keep their hints | `an_update_hints_added_members_and_unhints_removed_ones` |
| an abort that keeps the hints it added | `an_expired_reservation_is_recovered_by_the_next_writer`, `a_member_that_is_no_index_undoes_the_creation` |
| a removal that ignores the alias's protection | `a_hint_is_kept_while_the_alias_names_its_index_again` |
| recovery that ignores the deadline | `a_live_reservation_is_waited_for_then_left_alone`, `a_creation_still_pending_after_the_wait_reads_as_no_alias` |
| a deleted index dropped from an alias it is the last of | `a_deleted_index_leaves_its_aliases_but_the_last_keeps_naming_it` |
| `aliases_of` trusting the hint | `only_aliases_that_name_the_index_are_confirmed` |
| an addition already hinted left unswapped | `a_hinted_addition_is_swapped_again_to_fence_a_removal` |

Evidence for each criterion:

- AC-1: `creating_an_alias_hints_every_member_before_it_is_published`, `a_member_that_is_no_index_undoes_the_creation`, `an_update_hints_added_members_and_unhints_removed_ones`, `a_data_stream_keeps_its_fields_through_a_mutation`, `deleting_an_alias_unhints_its_members`, `an_update_cannot_rename_an_alias`
- AC-2: `an_expired_reservation_is_recovered_by_the_next_writer`, `an_abandoned_creation_is_deleted_by_the_next_writer`, `a_live_reservation_is_waited_for_then_left_alone`, `a_creation_still_pending_after_the_wait_reads_as_no_alias`, `a_reservation_carries_its_deadline`, `a_reservation_deadline_saturates`
- AC-3: `a_creation_removed_mid_preparation_is_superseded`, `a_reservation_taken_over_mid_preparation_is_superseded`, `a_failed_preparation_undoes_its_reservation`, `a_store_failure_mid_mutation_is_reported_and_undone`, `a_hint_is_kept_while_the_alias_names_its_index_again`, `a_hinted_addition_is_swapped_again_to_fence_a_removal`
- AC-4: `a_deleted_index_leaves_its_aliases_but_the_last_keeps_naming_it`, `an_alias_that_keeps_changing_is_counted_as_left`, `only_aliases_that_name_the_index_are_confirmed`
