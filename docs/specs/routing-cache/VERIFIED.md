# Routing cache: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-meta`
- the whole gate, `scripts/gate.sh`

`cargo mutants` over the diff: every mutant is caught. Tests were added for the ones it first missed: the prune's guard and boundary, the budget's boundaries, and forgetting a name found gone.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| a hit that ignores the window | `a_name_read_once_routes_without_a_request_for_the_window`, `another_nodes_change_routes_within_the_window`, `a_name_found_gone_is_forgotten` |
| a read kept after a racing local change | `a_read_racing_a_local_change_is_not_kept` |
| an update that does not tell the listener | `every_applied_change_is_told_and_no_other`, `this_nodes_own_changes_route_at_once` |
| an absent resolution kept | `an_alias_being_created_routes_as_absent_and_is_not_kept` |

Evidence for each criterion:

- AC-1: `a_name_read_once_routes_without_a_request_for_the_window`, `a_miss_is_never_cached`, `a_window_of_zero_reads_through_every_time`
- AC-2: `every_applied_change_is_told_and_no_other`, `this_nodes_own_changes_route_at_once`, `another_nodes_change_routes_within_the_window`, `a_read_racing_a_local_change_is_not_kept`, `a_name_found_gone_is_forgotten`
- AC-3: `the_cache_holds_its_byte_budget`, `expired_names_stay_until_they_are_in_the_way`, `past_the_threshold_expired_names_are_dropped_once_a_window`, `a_prune_comes_a_full_window_after_the_last`, `a_default_cache_is_a_new_one`
- AC-4: `an_alias_being_created_routes_as_absent_and_is_not_kept`, `a_malformed_or_oversized_register_is_refused_not_kept`
