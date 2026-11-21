# Verified

- AC-1: `several_parts_are_one_put_and_each_landing_names_its_bytes`, `a_landed_location_resolves_to_the_pack_key`
- AC-2: `a_taken_slot_is_refused_and_keeps_its_bytes`
- AC-3: `publishes_within_the_linger_share_one_pack`, `a_lone_publish_is_its_own_pack_after_the_linger`, `a_full_batch_is_written_without_waiting_out_the_linger`
- AC-4: `a_failed_pack_fails_its_publishes_and_the_ordinal_is_spent`
- AC-5: `the_epoch_is_recorded_once_before_the_first_pack`
- AC-6: `racing_epoch_records_both_land`
- AC-7: `a_64_mib_pack_writes_with_peak_heap_below_2_mib`
- AC-8: `the_next_epoch_is_past_every_epoch_in_the_ledger`
- AC-9: `a_leader_that_panics_leaves_its_followers_an_error_not_a_wait`
