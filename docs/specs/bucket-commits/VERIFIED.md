# Verified

- AC-1: `a_cold_swap_of_three_members_is_one_read_and_one_write`
- AC-2: `the_next_swap_reads_nothing`
- AC-3: `a_stale_member_conflicts_and_its_siblings_still_land`
- AC-4: `another_nodes_write_costs_one_read_and_one_retry`
- AC-5: `sixteen_lost_swaps_give_up`, `a_swap_that_loses_fewer_than_sixteen_times_lands`
- AC-6: `a_missing_bucket_is_an_error_and_create_does_not_overwrite`
- AC-7: `a_members_commit_is_read_without_the_rest`
- AC-8: `every_write_form_reads_back`, `auto_compresses_only_a_large_manifest`
- AC-9: `threads_swapping_different_members_all_land`
- AC-10: `a_commit_that_is_not_a_manifest_is_refused_before_any_request`
