# Tombstone sweep and the conditional-delete probe: verified

Run with `cargo test -p oss-meta`, `cargo test -p oss-format --test records` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox). Each listed defect was planted and seen to fail: a sweep that skips the deadline check (`a_claim_past_its_deadline_is_left_claimed`), and a probe that skips its delete at the stale generation (all four probe tests).

- AC-1: `every_backend_honours_a_conditional_delete`, `a_store_that_ignores_the_condition_gets_tombstones`, `any_failure_or_refusal_gets_tombstones`, `each_probe_uses_its_own_key`
- AC-2: `every_backend_honours_a_conditional_delete`
- AC-3: `a_tombstone_past_its_quarantine_is_removed_with_its_marker`, `a_store_without_conditional_deletes_keeps_every_tombstone`
- AC-4: `stale_markers_are_dropped_without_touching_the_register`, `another_sweeps_claim_is_kept_until_its_handoff`, `a_claim_past_its_deadline_is_left_claimed`, `a_tombstone_stamped_ahead_of_the_sweeps_clock_is_kept`
- AC-5: `buckets_not_owned_and_names_not_hours_are_skipped_unread`
- AC-6: `the_walk_stops_at_the_first_young_hour`, `a_tombstone_past_its_quarantine_is_removed_with_its_marker`, `stale_markers_are_dropped_without_touching_the_register`, `markers_are_listed_a_page_at_a_time`, `buckets_not_owned_and_names_not_hours_are_skipped_unread`
- AC-7: `a_bucket_name_gives_back_its_hour`
