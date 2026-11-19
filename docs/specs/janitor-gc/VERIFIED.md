# Janitor: reclaim segment blobs no commit names: verified

Run with `cargo test -p oss-janitor`, `cargo mutants -p oss-janitor` (32 mutants: 27 caught, 4 unviable, 1 missed, the `Display` of the error, then covered by `an_unreadable_pin_says_so`) and the whole gate, `scripts/gate.sh`, on 2026-11-19 (Linux sandbox, emulator only, numbers provisional).

- AC-1: `an_unreferenced_blob_of_an_older_term_goes_on_the_second_sweep_not_the_first`
- AC-2: `a_blob_the_manifest_names_is_never_deleted_whatever_its_term`
- AC-3: `a_blob_of_the_live_or_a_later_term_is_never_deleted`
- AC-4: `a_pinned_blob_stays_and_leaves_the_memory`
- AC-5: `pins_that_cannot_say_delete_nothing_and_keep_the_memory`
- AC-6: `a_shard_without_a_manifest_is_not_listed_and_a_bad_one_deletes_nothing`
- AC-7: `a_minimum_age_holds_a_blob_seen_twice_until_the_clock_has_moved`
- AC-8: `a_new_owner_with_an_empty_memory_deletes_nothing_on_its_first_sweep`
- AC-9: `a_dry_run_deletes_nothing_and_reports_what_it_would`
- AC-10: `keys_that_are_not_term_blobs_are_never_judged`, `what_is_not_a_term_blob_is_not_judged`
- AC-11: `a_sweep_is_one_register_read_a_listing_and_a_delete_batch`
- AC-12: `the_memory_holds_only_what_is_still_a_candidate`
- AC-13: `a_sweep_watches_no_more_than_its_cap_and_later_sweeps_take_the_rest`
