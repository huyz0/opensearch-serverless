# Reader budgets: verified

Run on 2026-11-16 in a Linux sandbox (emulator numbers, provisional).

- AC-1: `a_built_index_is_found_with_two_gets_and_served`
- AC-2: `a_full_window_stays_within_its_reservation` (debug build: the window took 1,582,000 B beyond a one-hit search, with 1,920,000 B now reserved for hits; before the fix 1,360,000 B were reserved and the test failed; whole peak 1,656,079 B; 429 with the window short of one byte)
- AC-3: `the_estimate_bounds_what_an_open_retains` (14 shapes, each searched before it is measured; the estimate is 1.44 to 3.7 times the retained heap on indexes over 128 KiB, and within the 128 KiB floor below it)
- AC-4: `a_file_is_whole_when_it_fits_a_page_or_cannot_be_paged`, `the_estimate_adds_the_shard_segments_fields_files_and_whole_files`, `a_memory_only_cache_pages_on_this_host_as_the_engine_does`
- AC-5: `an_open_index_holds_its_charge_until_the_last_search_ends`, `an_index_a_search_holds_is_not_closed`, `an_idle_index_that_cannot_make_room_is_not_closed`
- AC-6: `a_full_pool_closes_the_least_recently_searched_index`, `the_least_recently_found_closes_first`
- AC-7: `an_index_larger_than_the_pool_is_refused_and_closes_nothing`, `an_index_the_open_pool_cannot_hold_is_refused_with_429`, `a_named_breaker_names_itself_and_its_durability`
- AC-8: `concurrent_first_searches_open_once_and_close_nothing_else`, `a_second_search_waits_for_the_first_open`, `a_failed_open_lets_the_next_search_lead`
- AC-9: `a_node_with_no_open_index_pool_does_not_start`
