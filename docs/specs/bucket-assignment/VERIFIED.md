# Verified

- AC-1: `a_stripe_is_the_low_four_bits_of_the_groups_hash`
- AC-2: `the_first_join_creates_the_bucket`
- AC-3: `joining_twice_writes_nothing_the_second_time`
- AC-4: `a_groups_indices_share_a_bucket_one_conditional_write_each`
- AC-5: `a_full_bucket_moves_the_stripe_to_the_next_one`, `a_bucket_is_full_by_data_size_at_the_target`, `a_bucket_is_full_by_stored_manifest_size_at_the_limit`, `a_bucket_one_under_the_member_limit_still_takes_one`
- AC-6: `a_lost_manifest_swap_is_retried_and_sixteen_are_busy`
- AC-7: `leave_removes_only_that_member_and_tolerates_the_absent`
- AC-8: `limits_are_clamped`
- AC-9: `the_value_is_the_generation_in_decimal`, `whitespace_is_trimmed_and_anything_else_is_refused`, `the_key_is_under_the_open_prefix`, `the_open_register_round_trips_through_the_store`
