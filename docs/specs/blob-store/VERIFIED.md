# Blob store trait and in-process S3 emulator: verified

Run with `cargo test -p oss-blob --all-features` and the whole gate, `scripts/gate.sh`, on 2026-11-14 (Linux sandbox; emulator numbers are provisional). Each emulator test was also run against a planted defect (no `If-None-Match` check, a create checked only before its body streams, a lost response that does not fail, an off-by-one at the object end, a double count per attempt) and seen to fail.

- AC-1: `each_operation_records_exactly_one_attempt_of_its_kind`
- AC-2: `a_failed_attempt_is_still_one_attempt_and_one_error`, `a_sink_failure_ends_the_get_and_counts_as_an_error`
- AC-3: `a_get_streams_through_the_caller_buffer`
- AC-4: `a_ranged_get_returns_exactly_the_bytes_asked_for`, `any_range_of_an_empty_object_is_invalid`
- AC-5: `a_conditional_create_never_overwrites`, `a_create_of_a_taken_key_is_refused_before_capacity_is_checked`, `of_racing_creates_exactly_one_wins`
- AC-6: `a_put_with_a_bad_body_leaves_the_old_object`
- AC-7: `listing_pages_through_every_key_once`, `a_page_is_the_next_keys_in_byte_order`
- AC-8: `the_emulator_refuses_writes_past_its_capacity`
- AC-9: `the_same_seed_places_the_same_faults`, `scripted_faults_hit_exactly_the_next_requests_of_their_kind`
- AC-10: `injected_latency_delays_each_request_of_its_kind`
- AC-11: `invalid_arguments_fail_before_any_request`
- AC-13: `a_lost_response_still_commits_and_a_retried_create_finds_its_own_object`
- AC-12: `scripts/check-alloc.py`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`
