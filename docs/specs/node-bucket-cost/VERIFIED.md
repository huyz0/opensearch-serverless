# Verified

- AC-1: `an_index_created_with_the_switch_on_is_written_and_searched_through_its_bucket`, `the_same_requests_with_the_switch_off_leave_a_shard_register`
- AC-2: `small_indices_cost_no_more_requests_in_buckets_once_created`, and `a_search_window_may_hold_one_more_secret_read_but_not_two_or_a_ranged_read` for the comparison itself (seen red with the strict rule on CI's 37 against 38 plain reads). The extra plain read was traced to `TransportSecretStore::read_once` landing in the window (local run with a backtrace on every GET, provisional, emulator)
- AC-3: `small_indices_cost_no_more_requests_in_buckets_once_created`
- AC-4: `small_indices_cost_no_more_requests_in_buckets_once_created`
- AC-5: `taking_over_small_indices_costs_fewer_requests_in_buckets`
