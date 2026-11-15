# Memory budget and test allocator: verified

- AC-1: `take_up_to_the_limit_then_refuse_one_more_byte`, `reserve_exactly_free_bytes_then_refuse_one_more`
- AC-2: `drop_returns_the_bytes`, `pools_are_independent_and_drop_returns_bytes`
- AC-3: `pools_summing_to_the_total_are_accepted_one_more_byte_refused`, `overflowing_pool_sizes_are_refused`
- AC-4: `grow_to_the_limit_then_refuse_one_more_byte_unchanged`, `shrink_releases_at_once_and_clamps_to_what_is_held`
- AC-5: `construction_charges_size_times_count`, `construction_one_buffer_too_many_is_refused`, `charge_is_released_when_pool_and_guards_drop`
- AC-6: `count_checkouts_succeed_and_one_more_is_refused`, `returning_one_guard_frees_one_checkout`
- AC-7: `sixteen_threads_never_exceed_the_buffer_count_or_pool_limits`
- AC-8: `ten_thousand_checkouts_and_reservations_allocate_nothing`
- AC-9: `counting_allocator_reads_peaks_and_assertions_hold_at_the_bound`
- AC-10: `peak_heap_under_sustained_load_stays_within_the_budget` (debug: peak 2,098,210 bytes against a 2,097,152-byte budget plus 64 KiB allowance, 1,217 of 16,000 requests refused; with the reservation bypassed it read 3,790,531 and failed)
- AC-11: `scripts/check-layering.py`
